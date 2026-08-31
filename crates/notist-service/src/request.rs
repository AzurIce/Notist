use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use notist_analysis::{
    AnalyzerConfiguration, AnnotationEntry, CompletionKind, DiagnosticKind, DocumentVersions,
    ResourceFile, ResourceKind, SignatureSet, SourceOverlays, Value, WorkspaceSnapshot,
    WorkspaceSymbolKind,
};
use notist_html::{
    HtmlRendererRegistry, RenderOptions, RenderedAnnotation, module_anchors_tree,
    outline_entries_tree, register_web_component_renderer, render_element_tree_with_renderers,
};
use notist_model::{DefaultValue, FunctionSignature, ModulePath, Parameter, TextRange, Type};
use notist_syntax::Attribute;
use percent_encoding::{AsciiSet, CONTROLS, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};

use crate::{
    NotistService, SearchIndexBuild, ServiceViewId, SnapshotIdentity, VaultIdentity, ViewKind,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum CoreRequest {
    OpenView {
        root: PathBuf,
        kind: ProtocolViewKind,
    },
    CloseView {
        view_id: ServiceViewId,
    },
    UpdateView {
        view_id: ServiceViewId,
        documents: Vec<OverlayDocument>,
        configuration: Option<ConfigurationRecord>,
    },
    /// Incremental overlay update: upserts the given documents and removes
    /// the given paths from the view's overlay set, leaving every other
    /// overlay untouched. The full-replace `UpdateView` stays available for
    /// callers that want to define the whole overlay set.
    UpdateViewDelta {
        view_id: ServiceViewId,
        upsert: Vec<OverlayDocument>,
        remove: Vec<PathBuf>,
    },
    SnapshotSummary {
        view_id: ServiceViewId,
    },
    Status {
        view_id: ServiceViewId,
    },
    Sources {
        view_id: ServiceViewId,
    },
    /// Per-file line tables (byte offsets) instead of full text: enough for a
    /// UTF-8-negotiated LSP adapter to convert positions without pulling the
    /// whole vault's source across the wire.
    SourceTables {
        view_id: ServiceViewId,
    },
    ReloadDiskView {
        view_id: ServiceViewId,
    },
    Diagnostics {
        view_id: ServiceViewId,
    },
    DiagnosticsPage {
        view_id: ServiceViewId,
        query: crate::query::DiagnosticsQuery,
    },
    Inspect {
        view_id: ServiceViewId,
    },
    Definition {
        view_id: ServiceViewId,
        path: PathBuf,
        offset: usize,
    },
    References {
        view_id: ServiceViewId,
        path: PathBuf,
        offset: usize,
        include_definition: bool,
    },
    ReferencesTo {
        view_id: ServiceViewId,
        module: String,
        include_definition: bool,
    },
    /// Document-level references without position ambiguity: resolves the
    /// module owning `path` and returns every resolved reference to/from it
    /// (consumed by the obsidian-notist backlinks/outgoing panels; the
    /// `textDocument/references` position contract cannot express this).
    DocumentReferences {
        view_id: ServiceViewId,
        path: PathBuf,
        direction: crate::query::ReferenceDirection,
        include_definition: bool,
    },
    Completion {
        view_id: ServiceViewId,
        path: PathBuf,
        offset: usize,
    },
    Hover {
        view_id: ServiceViewId,
        path: PathBuf,
        offset: usize,
    },
    DocumentSymbols {
        view_id: ServiceViewId,
        path: PathBuf,
    },
    DefinitionLocation {
        view_id: ServiceViewId,
        query: crate::query::DefinitionQuery,
    },
    Items {
        view_id: ServiceViewId,
        query: crate::query::ItemsQuery,
    },
    Ancestors {
        view_id: ServiceViewId,
        query: crate::query::AncestorsQuery,
    },
    ReferencesPage {
        view_id: ServiceViewId,
        query: crate::query::ReferencesQuery,
    },
    WorkspaceSymbols {
        view_id: ServiceViewId,
        query: String,
    },
    Search {
        view_id: ServiceViewId,
        query: String,
    },
    SearchPage {
        view_id: ServiceViewId,
        query: crate::query::SearchQuery,
    },
    IndexStatus {
        view_id: ServiceViewId,
    },
    IndexRebuild {
        view_id: ServiceViewId,
        wait: bool,
    },
    RenderWorkspace {
        view_id: ServiceViewId,
    },
    /// Single-document render for the obsidian-notist preview view
    /// (`notist/renderDocument`): same pipeline as RenderWorkspace, one page.
    RenderDocument {
        view_id: ServiceViewId,
        path: PathBuf,
    },
    ResolveReference {
        view_id: ServiceViewId,
        source_module: String,
        url: String,
    },
}

impl CoreRequest {
    pub fn view_id(&self) -> Option<ServiceViewId> {
        match self {
            Self::OpenView { .. } => None,
            Self::CloseView { view_id }
            | Self::UpdateView { view_id, .. }
            | Self::UpdateViewDelta { view_id, .. }
            | Self::SnapshotSummary { view_id }
            | Self::Status { view_id }
            | Self::Sources { view_id }
            | Self::SourceTables { view_id }
            | Self::ReloadDiskView { view_id }
            | Self::Diagnostics { view_id }
            | Self::DiagnosticsPage { view_id, .. }
            | Self::ResolveReference { view_id, .. }
            | Self::Inspect { view_id }
            | Self::Definition { view_id, .. }
            | Self::DefinitionLocation { view_id, .. }
            | Self::References { view_id, .. }
            | Self::ReferencesTo { view_id, .. }
            | Self::DocumentReferences { view_id, .. }
            | Self::Completion { view_id, .. }
            | Self::Hover { view_id, .. }
            | Self::DocumentSymbols { view_id, .. }
            | Self::Items { view_id, .. }
            | Self::Ancestors { view_id, .. }
            | Self::ReferencesPage { view_id, .. }
            | Self::WorkspaceSymbols { view_id, .. }
            | Self::Search { view_id, .. }
            | Self::SearchPage { view_id, .. }
            | Self::IndexStatus { view_id }
            | Self::IndexRebuild { view_id, .. }
            | Self::RenderWorkspace { view_id }
            | Self::RenderDocument { view_id, .. } => Some(*view_id),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolViewKind {
    Disk,
    Session,
}

impl From<ProtocolViewKind> for ViewKind {
    fn from(value: ProtocolViewKind) -> Self {
        match value {
            ProtocolViewKind::Disk => Self::Disk,
            ProtocolViewKind::Session => Self::Session,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OverlayDocument {
    pub path: PathBuf,
    pub version: i64,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfigurationRecord {
    pub manifest_override: Option<String>,
    pub signatures: Vec<NamedSignatureRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NamedSignatureRecord {
    pub name: String,
    pub parameters: Vec<ParameterRecord>,
    pub trailing_content: Option<String>,
    pub result: TypeRecord,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParameterRecord {
    pub name: String,
    pub ty: TypeRecord,
    pub default: Option<DefaultRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum TypeRecord {
    None,
    Bool,
    Int,
    Float,
    String,
    Content,
    Function,
    Optional(Box<TypeRecord>),
    Inferred,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum DefaultRecord {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
}

impl ConfigurationRecord {
    fn into_analysis(self) -> AnalyzerConfiguration {
        let mut signatures = SignatureSet::with_builtins();
        for signature in self.signatures {
            signatures.insert(
                &signature.name,
                FunctionSignature {
                    parameters: signature
                        .parameters
                        .into_iter()
                        .map(|parameter| Parameter {
                            name: parameter.name,
                            ty: parameter.ty.into(),
                            default: parameter.default.map(Into::into),
                        })
                        .collect(),
                    trailing_content: signature.trailing_content,
                    result: signature.result.into(),
                },
            );
        }
        AnalyzerConfiguration {
            manifest_override: self.manifest_override.map(Arc::from),
            signatures,
            ..AnalyzerConfiguration::default()
        }
    }
}

impl From<TypeRecord> for Type {
    fn from(value: TypeRecord) -> Self {
        match value {
            TypeRecord::None => Self::None,
            TypeRecord::Bool => Self::Bool,
            TypeRecord::Int => Self::Int,
            TypeRecord::Float => Self::Float,
            TypeRecord::String => Self::String,
            TypeRecord::Content => Self::Content,
            TypeRecord::Function => Self::Function,
            TypeRecord::Optional(inner) => Self::Optional(Box::new((*inner).into())),
            TypeRecord::Inferred => Self::Inferred,
        }
    }
}

impl From<DefaultRecord> for DefaultValue {
    fn from(value: DefaultRecord) -> Self {
        match value {
            DefaultRecord::None => Self::None,
            DefaultRecord::Bool(value) => Self::Bool(value),
            DefaultRecord::Int(value) => Self::Int(value),
            DefaultRecord::Float(value) => Self::Float(value),
            DefaultRecord::String(value) => Self::String(value),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum CoreResponse {
    Opened {
        view_id: ServiceViewId,
        vault: VaultIdentity,
    },
    Closed,
    Updated,
    SnapshotSummary(SnapshotSummary),
    Status(crate::query::StatusRecord),
    Sources(Vec<SourceRecord>),
    SourceTables(Vec<SourceTableRecord>),
    Reloaded,
    Diagnostics(Vec<DiagnosticRecord>),
    DiagnosticsPage(crate::query::DiagnosticsResult),
    Inspect(InspectRecord),
    Definition(Option<LocationRecord>),
    DefinitionLocation(Option<crate::query::Location>),
    References(Vec<LocationRecord>),
    DocumentReferences(DocumentReferencesResult),
    Completion(Vec<CompletionRecord>),
    Hover(Option<HoverRecord>),
    DocumentSymbols(Vec<DocumentSymbolRecord>),
    Items(crate::query::QueryResult<crate::query::ItemRecord>),
    Ancestors(crate::query::QueryResult<crate::query::AncestorRecord>),
    ReferencesPage(crate::query::QueryResult<crate::query::ReferenceRecord>),
    WorkspaceSymbols(Vec<WorkspaceSymbolRecord>),
    ResolvedReference(RefTargetRecord),
    Search(Vec<SearchRecord>),
    SearchPage(crate::query::QueryResult<crate::query::SearchHit>),
    IndexStatus(crate::query::IndexStatusRecord),
    QueryError(crate::query::ToolError),
    RenderedWorkspace(RenderedWorkspaceRecord),
    RenderedDocument(RenderedDocumentRecord),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CoreReply {
    pub snapshot: SnapshotIdentity,
    pub response: CoreResponse,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotSummary {
    pub root: PathBuf,
    pub module_count: usize,
    pub source_count: usize,
    pub diagnostic_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceRecord {
    pub path: PathBuf,
    pub text: String,
    pub document_version: Option<i64>,
}

/// Byte-offset line table for one source (see `LineTable` in
/// notist-analysis): everything a UTF-8-negotiated adapter needs to convert
/// positions without the source text.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceTableRecord {
    pub path: PathBuf,
    pub document_version: Option<i64>,
    pub line_starts: Vec<u32>,
    pub content_ends: Vec<u32>,
    /// Total source length in bytes: the saturation target for positions
    /// past the last line.
    pub end: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ByteRange {
    pub start: usize,
    pub end: usize,
}

impl From<TextRange> for ByteRange {
    fn from(value: TextRange) -> Self {
        Self {
            start: value.start,
            end: value.end,
        }
    }
}

impl From<ByteRange> for TextRange {
    fn from(value: ByteRange) -> Self {
        TextRange::new(value.start, value.end)
    }
}

/// Discriminated reference target (D0004 RefTarget) in serialized form.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RefTargetRecord {
    /// module | scope | resource | external | missing
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// nonexistent | ambiguous | unsupported
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagnosticRecord {
    pub path: Option<PathBuf>,
    pub source: Option<String>,
    pub range: Option<ByteRange>,
    pub code: String,
    pub severity: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InspectRecord {
    pub modules: Vec<ModuleRecord>,
    pub references: Vec<ReferenceRecord>,
    pub semantic_records: Vec<SemanticRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SemanticRecord {
    pub module: String,
    pub range: ByteRange,
    pub kind: String,
    pub name: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModuleRecord {
    pub logical_path: String,
    pub source_path: Option<PathBuf>,
    pub virtual_module: bool,
    /// Root binding names (D0004 ModuleResult.bindings), sorted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<String>,
    /// Module attributes declared by `@![...]` (D0006), in source order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<AttributeRecord>,
}

/// One module or scope attribute set on the wire (D0006).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttributeRecord {
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<(String, String)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReferenceRecord {
    pub source_module: String,
    pub range: ByteRange,
    pub target_module: String,
    pub target_name: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocationRecord {
    pub path: PathBuf,
    pub source: String,
    pub range: ByteRange,
    pub is_definition: bool,
}

/// One occurrence in a `DocumentReferences` result: a reference to or from
/// the queried module, plus its resolved target identity. Ranges are
/// byte-based here; LSP clients convert them via each document's line index.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentReferenceItem {
    /// Source document of the occurrence.
    pub path: PathBuf,
    /// Byte range of the occurrence inside `path` (definition: module head).
    pub range: ByteRange,
    /// "incoming" | "outgoing" relative to the queried module.
    pub direction: String,
    pub source_module: String,
    pub target_module: String,
    pub target_name: Option<String>,
    /// Outgoing only: "module" | "scope" | "resource".
    pub target_kind: Option<String>,
    pub url: Option<String>,
    pub is_definition: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentReferencesResult {
    /// Snapshot revision that produced these records (freshness gating).
    pub revision: u64,
    pub items: Vec<DocumentReferenceItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompletionRecord {
    pub label: String,
    pub kind: String,
    pub detail: String,
    pub documentation: Option<String>,
    pub replacement: ByteRange,
    pub insert_text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HoverRecord {
    pub range: ByteRange,
    pub markdown: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentSymbolRecord {
    pub name: String,
    pub level: u8,
    pub range: ByteRange,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceSymbolRecord {
    pub name: String,
    pub kind: String,
    pub path: PathBuf,
    pub range: ByteRange,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchRecord {
    pub path: PathBuf,
    pub range: ByteRange,
    pub snippet: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenderedWorkspaceRecord {
    pub site_name: String,
    pub pages: Vec<RenderedPageRecord>,
    /// Analysis diagnostics captured from the same snapshot as the pages (D0010).
    pub analysis_diagnostics: Vec<DiagnosticRecord>,
    pub evaluation_diagnostics: Vec<DiagnosticRecord>,
    /// Every module resource file, copied wholesale by the build layer.
    pub resources: Vec<RenderedResourceRecord>,
}

/// One resource file to copy into the owning module's output directory.
/// One rendered document plus the module-scoped resource table, consumed by
/// the obsidian-notist preview view (notist/renderDocument). The snapshot
/// revision travels on `CoreReply.snapshot`, as with every reply.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenderedDocumentRecord {
    pub page: RenderedPageRecord,
    /// Resource files of the rendered module, for fragment URL rewriting.
    pub resources: Vec<RenderedResourceRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenderedResourceRecord {
    pub module_segments: Vec<String>,
    /// The real on-disk file name, used verbatim for the copied artifact.
    pub name: String,
    /// `"image"` or `"file"`.
    pub kind: String,
    pub source_path: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenderedPageRecord {
    pub module_segments: Vec<String>,
    pub fragment: String,
    /// Plain text of the first heading, or `None` when the page has no heading.
    pub title: Option<String>,
    /// Top-level headings of the page with their assigned HTML anchors.
    pub headings: Vec<RenderedHeadingRecord>,
    /// The module's root `let` bindings (own plus imported, D0004), sorted by
    /// name, for the preview inspector's symbol table.
    #[serde(default)]
    pub bindings: Vec<RenderedBindingRecord>,
    /// The module's raw source text from the rendered snapshot, or `None` for
    /// a virtual directory module. The preview page embeds it so the UI can
    /// toggle between the rendered document and the `.not` source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// One module root binding shown in the preview inspector.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RenderedBindingRecord {
    pub name: String,
    /// Compact type/value summary: a scalar literal, `Content`, or a
    /// `fn(...) -> R` signature.
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RenderedHeadingRecord {
    pub level: u8,
    pub id: String,
    pub text: String,
}

enum IndexBuildWait {
    Ready(Arc<crate::query::SearchIndex>),
    Failed(String),
    TimedOut,
}

impl NotistService {
    fn index_cache_key(identity: &SnapshotIdentity) -> String {
        format!(
            "{}:{}:{}",
            identity.vault.fingerprint, identity.view_kind, identity.source_fingerprint
        )
    }

    fn index_operation_handle(identity: &SnapshotIdentity) -> String {
        let vault = &identity.vault.fingerprint[..identity.vault.fingerprint.len().min(8)];
        let source = &identity.source_fingerprint[..identity.source_fingerprint.len().min(16)];
        format!("index-{vault}-{source}")
    }

    fn start_index_build(
        &self,
        workspace: &WorkspaceSnapshot,
        identity: &SnapshotIdentity,
        rebuild: bool,
    ) -> io::Result<Arc<SearchIndexBuild>> {
        let key = Self::index_cache_key(identity);
        let mut builds = self.search_index_builds.lock().unwrap();
        if let Some(existing) = builds.get(&key).cloned() {
            let pending = existing.result.lock().unwrap().is_none();
            if !rebuild || pending {
                return Ok(existing);
            }
            builds.remove(&key);
        }

        if rebuild {
            self.search_indexes.lock().unwrap().remove(&key);
            crate::query::SearchIndex::remove_stored(
                workspace.root(),
                &identity.source_fingerprint,
            )?;
        }

        let build = Arc::new(SearchIndexBuild {
            operation_handle: Self::index_operation_handle(identity),
            result: std::sync::Mutex::new(None),
            ready: std::sync::Condvar::new(),
        });
        builds.insert(key.clone(), build.clone());
        drop(builds);

        let captured = workspace.clone();
        let fingerprint = identity.source_fingerprint.clone();
        let indexes = self.search_indexes.clone();
        let task = build.clone();
        std::thread::spawn(move || {
            let started = Instant::now();
            let result = loop {
                match crate::query::SearchIndex::build(&captured, &fingerprint) {
                    Ok(index) => break Ok(Arc::new(index)),
                    Err(error)
                        if error.kind() == io::ErrorKind::WouldBlock
                            && started.elapsed() < Duration::from_secs(10) =>
                    {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(error) => break Err(error.to_string()),
                }
            };
            if let Ok(index) = &result {
                indexes.lock().unwrap().insert(key, index.clone());
            }
            *task.result.lock().unwrap() = Some(result);
            task.ready.notify_all();
        });
        Ok(build)
    }

    fn wait_for_index(build: &SearchIndexBuild, timeout: Option<Duration>) -> IndexBuildWait {
        let result = build.result.lock().unwrap();
        let result = if let Some(timeout) = timeout {
            let (result, timeout_result) = build
                .ready
                .wait_timeout_while(result, timeout, |result| result.is_none())
                .unwrap();
            if timeout_result.timed_out() && result.is_none() {
                return IndexBuildWait::TimedOut;
            }
            result
        } else {
            build
                .ready
                .wait_while(result, |result| result.is_none())
                .unwrap()
        };
        match result.as_ref().expect("completed index build has a result") {
            Ok(index) => IndexBuildWait::Ready(index.clone()),
            Err(error) => IndexBuildWait::Failed(error.clone()),
        }
    }

    fn index_status(
        &self,
        workspace: &WorkspaceSnapshot,
        identity: &SnapshotIdentity,
        not_built_message: &str,
    ) -> crate::query::IndexStatusRecord {
        let key = Self::index_cache_key(identity);
        if let Some(index) = self.search_indexes.lock().unwrap().get(&key).cloned() {
            return crate::query::IndexStatusRecord {
                health: "ready".into(),
                stamp: Some(index.stamp.clone()),
                unit_count: index.unit_count,
                operation_handle: None,
                message: None,
            };
        }
        if let Some(build) = self.search_index_builds.lock().unwrap().get(&key).cloned() {
            let result = build.result.lock().unwrap();
            return match result.as_ref() {
                None => crate::query::IndexStatusRecord {
                    health: "building".into(),
                    stamp: None,
                    unit_count: 0,
                    operation_handle: Some(build.operation_handle.clone()),
                    message: Some("the current snapshot index is being built".into()),
                },
                Some(Ok(index)) => crate::query::IndexStatusRecord {
                    health: "ready".into(),
                    stamp: Some(index.stamp.clone()),
                    unit_count: index.unit_count,
                    operation_handle: None,
                    message: None,
                },
                Some(Err(error)) => crate::query::IndexStatusRecord {
                    health: "error".into(),
                    stamp: None,
                    unit_count: 0,
                    operation_handle: Some(build.operation_handle.clone()),
                    message: Some(error.clone()),
                },
            };
        }
        if let Some(status) =
            crate::query::SearchIndex::stored_status(workspace.root(), &identity.source_fingerprint)
        {
            return status;
        }
        if let Some(status) = crate::query::SearchIndex::stale_stored_status(
            workspace.root(),
            &identity.source_fingerprint,
        ) {
            return status;
        }

        let prefix = format!("{}:{}:", identity.vault.fingerprint, identity.view_kind);
        if let Some(index) = self
            .search_indexes
            .lock()
            .unwrap()
            .iter()
            .filter(|(candidate, _)| candidate.starts_with(&prefix) && *candidate != &key)
            .map(|(_, index)| index.clone())
            .next()
        {
            return crate::query::IndexStatusRecord {
                health: "stale".into(),
                stamp: Some(index.stamp.clone()),
                unit_count: index.unit_count,
                operation_handle: None,
                message: Some("the available index belongs to an older source set".into()),
            };
        }
        crate::query::IndexStatusRecord {
            health: "not_built".into(),
            stamp: None,
            unit_count: 0,
            operation_handle: None,
            message: Some(not_built_message.into()),
        }
    }

    pub fn execute(&self, request: CoreRequest) -> io::Result<CoreReply> {
        self.execute_cancellable(request, &AtomicBool::new(false))
    }

    pub fn execute_cancellable(
        &self,
        request: CoreRequest,
        cancelled: &AtomicBool,
    ) -> io::Result<CoreReply> {
        if cancelled.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "request cancelled",
            ));
        }
        match request {
            CoreRequest::OpenView { root, kind } => {
                let (view_id, vault) = self.open_view(root, kind.into())?;
                let (snapshot, ()) = self.with_snapshot(view_id, |_| ())?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::Opened { view_id, vault },
                })
            }
            CoreRequest::CloseView { view_id } => {
                let (snapshot, ()) = self.with_snapshot(view_id, |_| ())?;
                self.close_view(view_id);
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::Closed,
                })
            }
            CoreRequest::UpdateView {
                view_id,
                documents,
                configuration,
            } => {
                let mut overlays = SourceOverlays::new();
                let mut versions = DocumentVersions::new();
                for document in documents {
                    overlays.insert(document.path.clone(), Arc::from(document.text));
                    versions.insert(document.path, document.version);
                }
                let snapshot = self.replace_view_inputs(
                    view_id,
                    overlays,
                    versions,
                    configuration.map(ConfigurationRecord::into_analysis),
                )?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::Updated,
                })
            }
            CoreRequest::UpdateViewDelta {
                view_id,
                upsert,
                remove,
            } => {
                let snapshot = self.merge_view_inputs(view_id, upsert, remove)?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::Updated,
                })
            }
            CoreRequest::SnapshotSummary { view_id } => {
                let (snapshot, summary) =
                    self.with_snapshot(view_id, |workspace| SnapshotSummary {
                        root: workspace.root().to_path_buf(),
                        module_count: workspace.modules().count(),
                        source_count: workspace.sources().count(),
                        diagnostic_count: workspace.diagnostics().len(),
                    })?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::SnapshotSummary(summary),
                })
            }
            CoreRequest::Status { view_id } => {
                let kind = self.view_kind(view_id)?;
                let (snapshot, result) =
                    self.with_snapshot_identity(view_id, |workspace, identity| {
                        let index = self.index_status(
                            workspace,
                            identity,
                            "the lexical index is built lazily on first search",
                        );
                        let status = crate::query::status(
                            workspace,
                            identity,
                            kind,
                            self.runtime_mode(),
                            index,
                        );
                        if serde_json::to_vec(&status)
                            .map(|value| value.len())
                            .unwrap_or(usize::MAX)
                            > 8 * 1024
                        {
                            Err(crate::query::ToolError::new(
                                "budget_exhausted",
                                "status metadata exceeds its fixed 8 KiB logical budget",
                            ))
                        } else {
                            Ok(status)
                        }
                    })?;
                Ok(CoreReply {
                    snapshot,
                    response: match result {
                        Ok(status) => CoreResponse::Status(status),
                        Err(error) => CoreResponse::QueryError(error),
                    },
                })
            }
            CoreRequest::Sources { view_id } => {
                let (snapshot, sources) = self.with_snapshot(view_id, |workspace| {
                    workspace
                        .sources()
                        .map(|source| SourceRecord {
                            path: source.canonical_path.clone(),
                            text: source.text.to_string(),
                            document_version: source.document_version,
                        })
                        .collect()
                })?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::Sources(sources),
                })
            }
            CoreRequest::SourceTables { view_id } => {
                let (snapshot, tables) = self.with_snapshot(view_id, |workspace| {
                    workspace
                        .sources()
                        .map(|source| {
                            let table = source.line_index.line_table(&source.text);
                            SourceTableRecord {
                                path: source.canonical_path.clone(),
                                document_version: source.document_version,
                                line_starts: table.starts.iter().copied().collect(),
                                content_ends: table.content_ends.iter().copied().collect(),
                                end: table.end,
                            }
                        })
                        .collect()
                })?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::SourceTables(tables),
                })
            }
            CoreRequest::ReloadDiskView { view_id } => {
                let snapshot = self.reload_disk_view(view_id)?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::Reloaded,
                })
            }
            CoreRequest::Diagnostics { view_id } => {
                let (snapshot, diagnostics) = self.with_snapshot(view_id, |workspace| {
                    workspace
                        .diagnostics()
                        .iter()
                        .map(|diagnostic| DiagnosticRecord {
                            path: diagnostic.source_path.clone(),
                            source: diagnostic.source_path.as_ref().and_then(|path| {
                                let file_id = workspace.file_id(path)?;
                                Some(workspace.source(file_id)?.text.to_string())
                            }),
                            range: diagnostic.range.map(Into::into),
                            code: diagnostic_code(&diagnostic.kind).into(),
                            severity: diagnostic.kind.severity_label().into(),
                            message: diagnostic.message.clone(),
                        })
                        .collect()
                })?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::Diagnostics(diagnostics),
                })
            }
            CoreRequest::DiagnosticsPage { view_id, query } => {
                let (snapshot, result) = self
                    .with_snapshot_identity(view_id, |workspace, identity| {
                        crate::query::diagnostics(workspace, identity, &query)
                    })?;
                Ok(CoreReply {
                    snapshot,
                    response: match result {
                        Ok(page) => CoreResponse::DiagnosticsPage(page),
                        Err(error) => CoreResponse::QueryError(error),
                    },
                })
            }
            CoreRequest::Inspect { view_id } => {
                let (snapshot, inspect) =
                    self.with_snapshot(view_id, |workspace| InspectRecord {
                        modules: workspace
                            .modules()
                            .map(|module| {
                                let mut bindings: Vec<String> = workspace
                                    .module_bindings(module.id)
                                    .map(|bindings| bindings.keys().cloned().collect())
                                    .unwrap_or_default();
                                bindings.sort();
                                ModuleRecord {
                                    logical_path: module.logical_path.to_string(),
                                    source_path: module.source_path.clone(),
                                    virtual_module: module.source_path.is_none(),
                                    bindings,
                                    attributes: attribute_records(
                                        workspace.module_attributes(module.id),
                                    ),
                                }
                            })
                            .collect(),
                        references: workspace
                            .references()
                            .iter()
                            .map(|reference| ReferenceRecord {
                                source_module: reference.source_module.to_string(),
                                range: reference.range.into(),
                                target_module: reference.target_module.to_string(),
                                target_name: reference.target_name.clone(),
                            })
                            .collect(),
                        semantic_records: workspace
                            .modules()
                            .flat_map(|module| {
                                let module_name = module.logical_path.to_string();
                                let mut records = Vec::new();
                                if let Some(parse) = &module.parse {
                                    records.extend(parse.annotations().into_iter().map(
                                        |annotation| {
                                            SemanticRecord {
                                                module: module_name.clone(),
                                                range: annotation.scope_range.into(),
                                                kind: "embedded".into(),
                                                name: annotation
                                                    .attributes
                                                    .id
                                                    .as_ref()
                                                    .map(|id| id.value.clone()),
                                            }
                                        },
                                    ));
                                    records.extend(parse.calls().into_iter().map(|call| {
                                        let (range, kind) = match call.trailing.first() {
                                            Some(body) => (body.payload_range, "content call"),
                                            None => (call.range, "call"),
                                        };
                                        SemanticRecord {
                                            module: module_name.clone(),
                                            range: range.into(),
                                            kind: kind.into(),
                                            name: Some(call.name.value.clone()),
                                        }
                                    }));
                                    records.extend(parse.raw_literals().into_iter().map(|raw| {
                                        let kind = match raw.form {
                                            notist_syntax::RawLiteralForm::Inline => "inline raw",
                                            notist_syntax::RawLiteralForm::Fenced => "fenced raw",
                                        };
                                        SemanticRecord {
                                            module: module_name.clone(),
                                            range: raw.payload_range.into(),
                                            kind: kind.into(),
                                            name: raw.tag.as_ref().map(|tag| tag.value.clone()),
                                        }
                                    }));
                                }
                                records
                            })
                            .collect(),
                    })?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::Inspect(inspect),
                })
            }
            CoreRequest::Definition {
                view_id,
                path,
                offset,
            } => {
                let (snapshot, definition) = self.with_snapshot(view_id, |workspace| {
                    let file_id = workspace.file_id(&path)?;
                    let definition = workspace.definition_at(file_id, offset)?;
                    let source = workspace.source(definition.file_id?)?;
                    Some(LocationRecord {
                        path: source.canonical_path.clone(),
                        source: source.text.to_string(),
                        range: definition.range.unwrap_or(TextRange::new(0, 0)).into(),
                        is_definition: true,
                    })
                })?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::Definition(definition),
                })
            }
            CoreRequest::References {
                view_id,
                path,
                offset,
                include_definition,
            } => {
                let (snapshot, references) = self.with_snapshot(view_id, |workspace| {
                    let Some(file_id) = workspace.file_id(&path) else {
                        return Vec::new();
                    };
                    workspace
                        .symbol_locations_at(file_id, offset, include_definition)
                        .into_iter()
                        .filter_map(|location| {
                            Some(LocationRecord {
                                path: workspace.source(location.file_id)?.canonical_path.clone(),
                                source: workspace.source(location.file_id)?.text.to_string(),
                                range: location.range.into(),
                                is_definition: location.is_definition,
                            })
                        })
                        .collect()
                })?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::References(references),
                })
            }
            CoreRequest::ReferencesTo {
                view_id,
                module,
                include_definition,
            } => {
                let (snapshot, references) = self.with_snapshot(view_id, |workspace| {
                    let Some(module_path) = parse_absolute_module_path(&module) else {
                        return Vec::new();
                    };
                    let Some(module) = workspace.module(&module_path) else {
                        return Vec::new();
                    };
                    let mut locations = Vec::new();
                    if include_definition
                        && let Some(file_id) = module.file_id
                        && let Some(source) = workspace.source(file_id)
                    {
                        locations.push(LocationRecord {
                            path: source.canonical_path.clone(),
                            source: source.text.to_string(),
                            range: TextRange::new(0, 0).into(),
                            is_definition: true,
                        });
                    }
                    locations.extend(workspace.references_to(module.id).filter_map(|reference| {
                        let source = workspace.source(reference.source_file_id)?;
                        Some(LocationRecord {
                            path: source.canonical_path.clone(),
                            source: source.text.to_string(),
                            range: reference.range.into(),
                            is_definition: false,
                        })
                    }));
                    locations
                })?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::References(references),
                })
            }
            CoreRequest::DocumentReferences {
                view_id,
                path,
                direction,
                include_definition,
            } => {
                let (snapshot, result) = self.with_snapshot(view_id, |workspace| {
                    let mut items = Vec::new();
                    if let Some(file_id) = workspace.file_id(&path)
                        && let Some(module) =
                            workspace.modules().find(|candidate| candidate.file_id == Some(file_id))
                    {
                        // Definition marker (module head); only meaningful
                        // for reference-to-module consumers.
                        if include_definition
                            && matches!(
                                direction,
                                crate::query::ReferenceDirection::Incoming
                                    | crate::query::ReferenceDirection::Both
                            )
                        {
                            items.push(DocumentReferenceItem {
                                path: path.clone(),
                                range: TextRange::new(0, 0).into(),
                                direction: "incoming".into(),
                                source_module: module.logical_path.to_string(),
                                target_module: module.logical_path.to_string(),
                                target_name: None,
                                target_kind: None,
                                url: None,
                                is_definition: true,
                            });
                        }
                        if matches!(
                            direction,
                            crate::query::ReferenceDirection::Incoming
                                | crate::query::ReferenceDirection::Both
                        ) {
                            for reference in workspace.references_to(module.id) {
                                let Some(source) = workspace.source(reference.source_file_id)
                                else {
                                    continue;
                                };
                                items.push(DocumentReferenceItem {
                                    path: source.canonical_path.clone(),
                                    range: reference.range.into(),
                                    direction: "incoming".into(),
                                    source_module: reference.source_module.to_string(),
                                    target_module: reference.target_module.to_string(),
                                    target_name: reference.target_name.clone(),
                                    target_kind: None,
                                    url: Some(reference.url.clone()),
                                    is_definition: false,
                                });
                            }
                        }
                        if matches!(
                            direction,
                            crate::query::ReferenceDirection::Outgoing
                                | crate::query::ReferenceDirection::Both
                        ) {
                            for reference in workspace.references().iter().filter(|reference| {
                                reference.source_module_id == module.id
                            }) {
                                let Some(source) = workspace.source(reference.source_file_id)
                                else {
                                    continue;
                                };
                                let target_kind =
                                    reference.target_name.as_deref().map_or("module", |name| {
                                        match workspace
                                            .resolve_item_name(&reference.target_module, name)
                                        {
                                            notist_analysis::RefTarget::Item {
                                                kind: notist_analysis::ItemKind::Resource(_),
                                                ..
                                            } => "resource",
                                            _ => "scope",
                                        }
                                    });
                                items.push(DocumentReferenceItem {
                                    path: source.canonical_path.clone(),
                                    range: reference.range.into(),
                                    direction: "outgoing".into(),
                                    source_module: module.logical_path.to_string(),
                                    target_module: reference.target_module.to_string(),
                                    target_name: reference.target_name.clone(),
                                    target_kind: Some(target_kind.into()),
                                    url: Some(reference.url.clone()),
                                    is_definition: false,
                                });
                            }
                        }
                    }
                    DocumentReferencesResult {
                        revision: workspace.revision().raw(),
                        items,
                    }
                })?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::DocumentReferences(result),
                })
            }
            CoreRequest::Completion {
                view_id,
                path,
                offset,
            } => {
                let (snapshot, completion) = self.with_snapshot(view_id, |workspace| {
                    let Some(file_id) = workspace.file_id(&path) else {
                        return Vec::new();
                    };
                    workspace
                        .completions_at(file_id, offset)
                        .into_iter()
                        .map(|candidate| CompletionRecord {
                            label: candidate.label,
                            kind: completion_kind(candidate.kind).into(),
                            detail: candidate.detail,
                            documentation: candidate.documentation,
                            replacement: candidate.replacement.into(),
                            insert_text: candidate.insert_text,
                        })
                        .collect()
                })?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::Completion(completion),
                })
            }
            CoreRequest::Hover {
                view_id,
                path,
                offset,
            } => {
                let (snapshot, hover) = self.with_snapshot(view_id, |workspace| {
                    let file_id = workspace.file_id(&path)?;
                    let hover = workspace.hover_at(file_id, offset)?;
                    Some(HoverRecord {
                        range: hover.range.into(),
                        markdown: hover.contents,
                    })
                })?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::Hover(hover),
                })
            }
            CoreRequest::DocumentSymbols { view_id, path } => {
                let (snapshot, symbols) = self.with_snapshot(view_id, |workspace| {
                    let Some(file_id) = workspace.file_id(&path) else {
                        return Vec::new();
                    };
                    workspace
                        .document_symbols(file_id)
                        .into_iter()
                        .map(|symbol| DocumentSymbolRecord {
                            name: symbol.name,
                            level: symbol.level,
                            range: symbol.range.into(),
                        })
                        .collect()
                })?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::DocumentSymbols(symbols),
                })
            }
            CoreRequest::Items { view_id, query } => {
                let (snapshot, result) = self
                    .with_snapshot_identity(view_id, |workspace, identity| {
                        crate::query::items(workspace, identity, &query)
                    })?;
                Ok(CoreReply {
                    snapshot,
                    response: match result {
                        Ok(page) => CoreResponse::Items(page),
                        Err(error) => CoreResponse::QueryError(error),
                    },
                })
            }
            CoreRequest::DefinitionLocation { view_id, query } => {
                let (snapshot, result) = self.with_snapshot(view_id, |workspace| {
                    crate::query::definition(workspace, &query)
                })?;
                Ok(CoreReply {
                    snapshot,
                    response: match result {
                        Ok(location) => CoreResponse::DefinitionLocation(location),
                        Err(error) => CoreResponse::QueryError(error),
                    },
                })
            }
            CoreRequest::Ancestors { view_id, query } => {
                let (snapshot, result) = self
                    .with_snapshot_identity(view_id, |workspace, identity| {
                        crate::query::ancestors(workspace, identity, &query)
                    })?;
                Ok(CoreReply {
                    snapshot,
                    response: match result {
                        Ok(page) => CoreResponse::Ancestors(page),
                        Err(error) => CoreResponse::QueryError(error),
                    },
                })
            }
            CoreRequest::ReferencesPage { view_id, query } => {
                let (snapshot, result) = self
                    .with_snapshot_identity(view_id, |workspace, identity| {
                        crate::query::references(workspace, identity, &query)
                    })?;
                Ok(CoreReply {
                    snapshot,
                    response: match result {
                        Ok(page) => CoreResponse::ReferencesPage(page),
                        Err(error) => CoreResponse::QueryError(error),
                    },
                })
            }
            CoreRequest::ResolveReference {
                view_id,
                source_module,
                url,
            } => {
                let (snapshot, result) = self.with_snapshot(view_id, |workspace| {
                    let source = crate::query::parse_absolute_module_path(&source_module)
                        .ok_or_else(|| {
                            crate::query::ToolError::new(
                                "invalid_selector",
                                "source module must be an absolute ModulePath",
                            )
                        })?;
                    Ok::<_, crate::query::ToolError>(crate::query::ref_target_record(
                        workspace,
                        workspace.resolve_reference(&source, &url),
                    ))
                })?;
                Ok(CoreReply {
                    snapshot,
                    response: match result {
                        Ok(record) => CoreResponse::ResolvedReference(record),
                        Err(error) => CoreResponse::QueryError(error),
                    },
                })
            }
            CoreRequest::WorkspaceSymbols { view_id, query } => {
                let (snapshot, symbols) = self.with_snapshot(view_id, |workspace| {
                    workspace
                        .workspace_symbols(&query)
                        .into_iter()
                        .filter_map(|symbol| {
                            Some(WorkspaceSymbolRecord {
                                name: symbol.name,
                                kind: match symbol.kind {
                                    WorkspaceSymbolKind::Module => "module",
                                    WorkspaceSymbolKind::Annotation => "annotation",
                                }
                                .into(),
                                path: workspace.source(symbol.file_id)?.canonical_path.clone(),
                                range: symbol.range.into(),
                            })
                        })
                        .collect()
                })?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::WorkspaceSymbols(symbols),
                })
            }
            CoreRequest::Search { view_id, query } => {
                let (snapshot, results) = self.with_snapshot(view_id, |workspace| {
                    workspace
                        .search_context_cancellable(&query, || cancelled.load(Ordering::Acquire))
                        .into_iter()
                        .filter_map(|result| {
                            Some(SearchRecord {
                                path: workspace.source(result.file_id)?.canonical_path.clone(),
                                range: result.range.into(),
                                snippet: result.snippet,
                            })
                        })
                        .collect()
                })?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::Search(results),
                })
            }
            CoreRequest::SearchPage { view_id, query } => {
                let (snapshot, result) =
                    self.with_snapshot_identity(view_id, |workspace, identity| match query.mode {
                        crate::query::SearchMode::Exact | crate::query::SearchMode::Regex => {
                            crate::query::exact_or_regex_search(workspace, identity, &query)
                        }
                        crate::query::SearchMode::Lexical | crate::query::SearchMode::Fuzzy => {
                            let key = Self::index_cache_key(identity);
                            let index = if let Some(index) = self
                                .search_indexes
                                .lock()
                                .unwrap()
                                .get(&key)
                                .cloned()
                            {
                                IndexBuildWait::Ready(index)
                            } else {
                                match self.start_index_build(workspace, identity, false) {
                                    Ok(build) => Self::wait_for_index(
                                        &build,
                                        Some(Duration::from_millis(query.wait_index_ms)),
                                    ),
                                    Err(error) => IndexBuildWait::Failed(error.to_string()),
                                }
                            };
                            match index {
                                IndexBuildWait::Ready(index) => {
                                    index.search(workspace, identity, &query)
                                }
                                IndexBuildWait::Failed(error) => Err(crate::query::ToolError::new(
                                    "index_not_ready",
                                    error,
                                )
                                .retryable("run `notist index rebuild`, retry, or use --exact")),
                                IndexBuildWait::TimedOut => {
                                    let handle = Self::index_operation_handle(identity);
                                    Err(crate::query::ToolError::new(
                                        "index_not_ready",
                                        format!(
                                            "index build did not finish within {} ms (operation {handle})",
                                            query.wait_index_ms
                                        ),
                                    )
                                    .retryable(
                                        "inspect `notist index status`, retry, or use --exact",
                                    ))
                                }
                            }
                        }
                    })?;
                Ok(CoreReply {
                    snapshot,
                    response: match result {
                        Ok(page) => CoreResponse::SearchPage(page),
                        Err(error) => CoreResponse::QueryError(error),
                    },
                })
            }
            CoreRequest::IndexStatus { view_id } => {
                let (snapshot, status) =
                    self.with_snapshot_identity(view_id, |workspace, identity| {
                        self.index_status(
                            workspace,
                            identity,
                            "run `notist index rebuild` or perform lexical search",
                        )
                    })?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::IndexStatus(status),
                })
            }
            CoreRequest::IndexRebuild { view_id, wait } => {
                let (snapshot, result) =
                    self.with_snapshot_identity(view_id, |workspace, identity| {
                        let build = self.start_index_build(workspace, identity, true)?;
                        if !wait {
                            return Ok(crate::query::IndexStatusRecord {
                                health: "building".into(),
                                stamp: None,
                                unit_count: 0,
                                operation_handle: Some(build.operation_handle.clone()),
                                message: Some("index rebuild submitted in the background".into()),
                            });
                        }
                        match Self::wait_for_index(&build, None) {
                            IndexBuildWait::Ready(index) => Ok(crate::query::IndexStatusRecord {
                                health: "ready".into(),
                                stamp: Some(index.stamp.clone()),
                                unit_count: index.unit_count,
                                operation_handle: Some(build.operation_handle.clone()),
                                message: Some("index rebuild completed".into()),
                            }),
                            IndexBuildWait::Failed(error) => Err(io::Error::other(error)),
                            IndexBuildWait::TimedOut => {
                                unreachable!("unbounded index wait timed out")
                            }
                        }
                    })?;
                Ok(CoreReply {
                    snapshot,
                    response: match result {
                        Ok(status) => CoreResponse::IndexStatus(status),
                        Err(error) => CoreResponse::QueryError(
                            crate::query::ToolError::new("index_not_ready", error.to_string())
                                .retryable("retry after correcting the reported index error"),
                        ),
                    },
                })
            }
            CoreRequest::RenderWorkspace { view_id } => {
                let (snapshot, rendered) = self
                    .with_snapshot(view_id, |workspace| render_workspace(workspace, cancelled))?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::RenderedWorkspace(rendered?),
                })
            }
            CoreRequest::RenderDocument { view_id, path } => {
                let (snapshot, rendered) = self.with_snapshot(view_id, |workspace| {
                    render_document(workspace, &path, cancelled)
                })?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::RenderedDocument(rendered?),
                })
            }
        }
    }
}

pub fn write_artifact_atomic(path: &Path, contents: &[u8], operation_id: &str) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("edited source has no parent directory"))?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("edited source has no file name"))?
        .to_string_lossy();
    let temporary = parent.join(format!(".{name}.notist-{operation_id}.tmp"));
    std::fs::write(&temporary, contents)?;
    if path.exists() {
        replace_file_platform(&temporary, path)
    } else {
        std::fs::rename(temporary, path)
    }
}

#[cfg(not(windows))]
fn replace_file_platform(temporary: &Path, target: &Path) -> io::Result<()> {
    std::fs::rename(temporary, target)
}

#[cfg(windows)]
fn replace_file_platform(temporary: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

    let temporary = temporary
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let target = target
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let replaced = unsafe {
        ReplaceFileW(
            target.as_ptr(),
            temporary.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if replaced == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

const URL_PATH_SEGMENT_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'%')
    .add(b'/')
    .add(b'\\')
    .add(b'?')
    .add(b'#')
    .add(b'<')
    .add(b'>')
    .add(b'"');

/// Everything fragment rendering needs that does not depend on which page is
/// being serialized; shared by whole-workspace renders (build/preview) and
/// the single-document render (`RenderDocument`).
struct RenderPrecompute<'a> {
    site_name: String,
    modules: Vec<&'a notist_analysis::Module>,
    known: std::collections::BTreeSet<ModulePath>,
    renderers: HtmlRendererRegistry,
    /// Per-source-module structured result, rendered annotations, outline
    /// headings and root bindings; `None` for virtual (source-less) modules,
    /// aligned with `modules` by index.
    prepared: Vec<Option<PreparedModule>>,
    /// Label → anchor id per module, for cross-module `module#label` links.
    anchor_maps: BTreeMap<ModulePath, BTreeMap<String, String>>,
    titles: BTreeMap<ModulePath, String>,
    resource_names: BTreeMap<&'a ModulePath, BTreeSet<&'a str>>,
    resources: Vec<RenderedResourceRecord>,
    evaluation_diagnostics: Vec<DiagnosticRecord>,
}

struct PreparedModule {
    structured: notist_analysis::StructuredModule,
    annotations: Vec<RenderedAnnotation>,
    headings: Vec<RenderedHeadingRecord>,
    bindings: Vec<RenderedBindingRecord>,
}

/// Precomputes the shared render state for one pass. Plugin packages are
/// composed once per pass and shared by every module; per-module reloading
/// would re-run the Wasm backend lifecycle N times.
fn precompute_render<'a>(
    workspace: &'a notist_analysis::WorkspaceSnapshot,
    cancelled: &AtomicBool,
) -> io::Result<RenderPrecompute<'a>> {
    let _span =
        tracing::debug_span!(target: "notist_service", "render_precompute", revision = workspace.revision().raw())
            .entered();
    let modules = workspace.modules().collect::<Vec<_>>();
    let known = modules
        .iter()
        .map(|module| module.logical_path.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let site_name = workspace
        .root()
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Notist".into());
    let mut evaluation_diagnostics = Vec::new();

    let mut renderers = HtmlRendererRegistry::default();
    for contribution in workspace.html_contributions() {
        if let Some(component) = &contribution.web_component {
            register_web_component_renderer(&mut renderers, &contribution.element, &component.tag);
        }
    }

    // Precompute every module's annotations and label-to-anchor mapping so that
    // reference resolution and fragment rendering share one anchor assignment.
    let runtime_plugins = workspace.runtime_plugins()?;
    let mut prepared = Vec::with_capacity(modules.len());
    let mut anchor_maps = BTreeMap::new();
    let mut titles = BTreeMap::new();
    for module in &modules {
        if cancelled.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "request cancelled",
            ));
        }
        let mut module_prepared = None;
        if let (Some(source_path), Some(source)) = (&module.source_path, module.source.as_deref()) {
            let structured = workspace
                .structured_module_with_runtime_plugins(module.id, &runtime_plugins)
                .expect("source-backed modules have structured results");
            let annotations = rendered_annotations(&structured.annotations);
            let anchors = module_anchors_tree(&structured.tree, &annotations);
            anchor_maps.insert(
                module.logical_path.clone(),
                anchors.into_iter().collect::<BTreeMap<_, _>>(),
            );
            let headings = outline_entries_tree(&structured.tree, &annotations)
                .into_iter()
                .map(|heading| RenderedHeadingRecord {
                    level: heading.level,
                    id: heading.id,
                    text: heading.text,
                })
                .collect::<Vec<_>>();
            if let Some(title) = headings.first() {
                titles.insert(module.logical_path.clone(), title.text.clone());
            }
            let mut bindings: Vec<RenderedBindingRecord> = workspace
                .module_bindings(module.id)
                .map(|module_bindings| {
                    module_bindings
                        .iter()
                        .map(|(name, value)| RenderedBindingRecord {
                            name: name.clone(),
                            detail: binding_detail(value),
                        })
                        .collect()
                })
                .unwrap_or_default();
            bindings.sort_by(|a, b| a.name.cmp(&b.name));
            for diagnostic in &structured.diagnostics {
                evaluation_diagnostics.push(DiagnosticRecord {
                    path: Some(source_path.clone()),
                    source: Some(source.to_owned()),
                    range: Some(diagnostic.range.into()),
                    code: "evaluation".into(),
                    severity: "error".into(),
                    message: diagnostic.message.clone(),
                });
            }
            module_prepared = Some(PreparedModule {
                structured,
                annotations,
                headings,
                bindings,
            });
        }
        prepared.push(module_prepared);
    }

    // Resource files resolve by exact file name after anchors; the build layer
    // copies every resource next to its module page.
    let mut resource_names: BTreeMap<&ModulePath, BTreeSet<&str>> = BTreeMap::new();
    let mut resources = Vec::new();
    for module in &modules {
        if module.resources.is_empty() {
            continue;
        }
        resource_names.insert(
            &module.logical_path,
            module
                .resources
                .iter()
                .map(|resource| resource.name.as_str())
                .collect(),
        );
        resources.extend(module_resources(module));
    }

    Ok(RenderPrecompute {
        site_name,
        modules,
        known,
        renderers,
        prepared,
        anchor_maps,
        titles,
        resource_names,
        resources,
        evaluation_diagnostics,
    })
}

/// Renders one module's page: the evaluated fragment for a source-backed
/// module, or the virtual-module stub. Cross-module link resolution reads
/// `precompute`'s anchor and resource tables.
fn render_module_page(
    module: &notist_analysis::Module,
    prepared: Option<&PreparedModule>,
    precompute: &RenderPrecompute,
) -> RenderedPageRecord {
    let Some(prepared) = prepared else {
        return RenderedPageRecord {
            module_segments: module.logical_path.segments().to_vec(),
            fragment: virtual_module_fragment(
                &module.logical_path,
                &precompute.modules,
                &precompute.titles,
                &module.resources,
            ),
            title: None,
            headings: Vec::new(),
            bindings: Vec::new(),
            source: None,
        };
    };
    let current = &module.logical_path;
    let resolver = |target: &ModulePath, label: Option<&str>| {
        if !precompute.known.contains(target) {
            return None;
        }
        match label {
            None => Some(module_href(current, target, None)),
            Some(label) => precompute
                .anchor_maps
                .get(target)
                .and_then(|anchors| anchors.get(label))
                .map(|anchor| module_href(current, target, Some(anchor)))
                .or_else(|| {
                    precompute
                        .resource_names
                        .get(target)
                        .filter(|names| names.contains(label))
                        .map(|_| resource_href(current, target, label))
                }),
        }
    };
    let fragment = render_element_tree_with_renderers(
        &prepared.structured.tree,
        &RenderOptions {
            current_module: Some(current),
            module_url_prefix: "",
        },
        Some(&resolver),
        &prepared.annotations,
        &precompute.renderers,
    );
    RenderedPageRecord {
        module_segments: module.logical_path.segments().to_vec(),
        fragment,
        title: prepared.headings.first().map(|heading| heading.text.clone()),
        headings: prepared.headings.clone(),
        bindings: prepared.bindings.clone(),
        source: module.source.as_deref().map(str::to_owned),
    }
}

fn render_workspace(
    workspace: &notist_analysis::WorkspaceSnapshot,
    cancelled: &AtomicBool,
) -> io::Result<RenderedWorkspaceRecord> {
    let _span =
        tracing::debug_span!(target: "notist_service", "render_workspace", revision = workspace.revision().raw())
            .entered();
    let precompute = precompute_render(workspace, cancelled)?;
    let mut pages = Vec::with_capacity(precompute.modules.len());
    for (module, prepared) in precompute.modules.iter().zip(&precompute.prepared) {
        if cancelled.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "request cancelled",
            ));
        }
        pages.push(render_module_page(module, prepared.as_ref(), &precompute));
    }
    // Analysis diagnostics are captured from the same snapshot the pages
    // were rendered from, so build/preview never read a newer revision (D0010).
    let analysis_diagnostics = workspace
        .diagnostics()
        .iter()
        .map(|diagnostic| DiagnosticRecord {
            path: diagnostic.source_path.clone(),
            source: diagnostic.source_path.as_ref().and_then(|path| {
                let file_id = workspace.file_id(path)?;
                Some(workspace.source(file_id)?.text.to_string())
            }),
            range: diagnostic.range.map(Into::into),
            code: diagnostic_code(&diagnostic.kind).into(),
            severity: diagnostic.kind.severity_label().into(),
            message: diagnostic.message.clone(),
        })
        .collect();
    Ok(RenderedWorkspaceRecord {
        site_name: precompute.site_name,
        pages,
        analysis_diagnostics,
        evaluation_diagnostics: precompute.evaluation_diagnostics,
        resources: precompute.resources,
    })
}

/// Single-document variant of `render_workspace` (`notist/renderDocument`):
/// the same cross-module anchor precompute, one serialized page. `path`
/// selects the module by its source file (same lookup the position requests
/// use); NotFound means the path backs no module.
fn render_document(
    workspace: &notist_analysis::WorkspaceSnapshot,
    path: &Path,
    cancelled: &AtomicBool,
) -> io::Result<RenderedDocumentRecord> {
    let _span =
        tracing::debug_span!(target: "notist_service", "render_document", revision = workspace.revision().raw())
            .entered();
    let precompute = precompute_render(workspace, cancelled)?;
    let file_id = workspace.file_id(path);
    let index = precompute
        .modules
        .iter()
        .position(|module| {
            module.file_id.is_some() && module.file_id == file_id
                || module.source_path.as_deref() == Some(path)
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("no module for source `{}`", path.display()),
            )
        })?;
    let module = precompute.modules[index];
    let page = render_module_page(module, precompute.prepared[index].as_ref(), &precompute);
    let resources = precompute
        .resources
        .iter()
        .filter(|resource| {
            resource.module_segments == module.logical_path.segments().to_vec()
        })
        .cloned()
        .collect();
    Ok(RenderedDocumentRecord { page, resources })
}

/// Wire projection of one module's resource files.
fn module_resources(module: &notist_analysis::Module) -> Vec<RenderedResourceRecord> {
    module
        .resources
        .iter()
        .map(|resource| RenderedResourceRecord {
            module_segments: module.logical_path.segments().to_vec(),
            name: resource.name.clone(),
            kind: resource_kind_name(resource.kind).to_owned(),
            source_path: resource.path.clone(),
        })
        .collect()
}

/// Projects `@![...]` module attributes onto the wire record (D0006).
pub(crate) fn attribute_records(attributes: &[notist_syntax::Attributes]) -> Vec<AttributeRecord> {
    attributes
        .iter()
        .map(|attributes| {
            let mut tags = Vec::new();
            let mut classes = Vec::new();
            let mut properties = Vec::new();
            for item in &attributes.items {
                match item {
                    Attribute::Class(name) => classes.push(name.value.clone()),
                    Attribute::Tag(name) => tags.push(name.value.clone()),
                    Attribute::KeyValue { key, value, .. } => {
                        properties.push((key.value.clone(), value.text().to_owned()));
                    }
                }
            }
            AttributeRecord {
                id: attributes.id.as_ref().map(|id| id.value.clone()),
                tags,
                classes,
                properties,
            }
        })
        .collect()
}

/// Projects the evaluation annotation table (postfix `@...` and block-prefix
/// `@[...]`, D0002/D0006) onto renderer annotations.
fn rendered_annotations(entries: &[AnnotationEntry]) -> Vec<RenderedAnnotation> {
    entries
        .iter()
        .map(|entry| {
            let mut classes = Vec::new();
            let mut tags = Vec::new();
            let mut properties = Vec::new();
            for item in &entry.attributes.items {
                match item {
                    Attribute::Class(name) => classes.push(name.value.clone()),
                    Attribute::Tag(name) => tags.push(name.value.clone()),
                    Attribute::KeyValue { key, value, .. } => {
                        properties.push((key.value.clone(), value.text().to_owned()));
                    }
                }
            }
            RenderedAnnotation {
                scope: entry.range,
                id: entry.attributes.id.as_ref().map(|id| id.value.clone()),
                classes,
                tags,
                properties,
            }
        })
        .collect()
}

/// Compact one-line summary of a root binding for the preview inspector:
/// scalar literals with their value, `Content`, or a `fn(...) -> R` signature.
fn binding_detail(value: &Value) -> String {
    match value {
        Value::None => "None".into(),
        Value::Bool(value) => format!("Bool = {value}"),
        Value::Int(value) => format!("Int = {value}"),
        Value::Float(value) => format!("Float = {value}"),
        Value::String(value) => format!("String = {}", truncated_string(value)),
        Value::Content(_) => "Content".into(),
        Value::Function(function) => format_signature(&function.signature),
        Value::Target(reference) => {
            let mut detail = reference.module.to_string();
            if let Some(name) = &reference.name {
                detail.push('/');
                detail.push_str(name);
            }
            format!("Target = {detail}")
        }
    }
}

/// Renders a string literal for the inspector: first line only, escaped and
/// truncated so multi-line or long strings stay on one line.
fn truncated_string(value: &str) -> String {
    let first_line = value.lines().next().unwrap_or("");
    let mut output = String::from("\"");
    for character in first_line.chars().take(24) {
        match character {
            '\\' => output.push_str("\\\\"),
            '"' => output.push_str("\\\""),
            _ => output.push(character),
        }
    }
    if first_line.chars().count() > 24 || value.lines().count() > 1 {
        output.push('…');
    }
    output.push('"');
    output
}

/// The D0007 written form of a function signature: `fn(x: Int) -> Int`, with
/// ` =` marking defaulted parameters and `trailing` the Content parameter.
fn format_signature(signature: &FunctionSignature) -> String {
    let mut output = String::from("fn(");
    for (index, parameter) in signature.parameters.iter().enumerate() {
        if index > 0 {
            output.push_str(", ");
        }
        if signature.trailing_content.as_deref() == Some(parameter.name.as_str()) {
            output.push_str("trailing ");
        }
        output.push_str(&parameter.name);
        output.push_str(": ");
        output.push_str(&parameter.ty.to_string());
        if parameter.default.is_some() {
            output.push_str(" =");
        }
    }
    if signature.result != Type::Inferred {
        output.push_str(") -> ");
        output.push_str(&signature.result.to_string());
    } else {
        output.push(')');
    }
    output
}

fn virtual_module_fragment(
    current: &ModulePath,
    modules: &[&notist_analysis::Module],
    titles: &BTreeMap<ModulePath, String>,
    resources: &[ResourceFile],
) -> String {
    let mut output = String::from("<h1>");
    if let Some(name) = current.segments().last() {
        escape_html(&mut output, name);
    } else {
        output.push_str("Home");
    }
    output.push_str("</h1><ul class=\"module-index\">");
    let child_depth = current.segments().len() + 1;
    for module in modules {
        let path = &module.logical_path;
        if path.segments().len() == child_depth && path.segments().starts_with(current.segments()) {
            output.push_str("<li><a href=\"");
            escape_attribute(&mut output, &module_href(current, path, None));
            output.push_str("\">");
            let name = titles
                .get(path)
                .map(String::as_str)
                .unwrap_or_else(|| path.segments().last().expect("child has a name"));
            escape_html(&mut output, name);
            output.push_str("</a></li>");
        }
    }
    output.push_str("</ul>");
    if !resources.is_empty() {
        // Resource files live next to this index page; the file name is both
        // the link text and the (percent-encoded) relative URL.
        output.push_str(
            "<h2 class=\"module-index-resources-title\">Resources</h2><ul class=\"module-index module-resource-index\">",
        );
        for resource in resources {
            output.push_str("<li><a href=\"");
            let encoded = utf8_percent_encode(&resource.name, URL_PATH_SEGMENT_ENCODE_SET)
                .collect::<String>();
            escape_attribute(&mut output, &encoded);
            output.push_str("\" data-notist-kind=\"");
            output.push_str(resource_kind_name(resource.kind));
            output.push_str("\">");
            escape_html(&mut output, &resource.name);
            output.push_str("</a></li>");
        }
        output.push_str("</ul>");
    }
    output
}

/// The protocol-level resource kind string.
fn resource_kind_name(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Image => "image",
        ResourceKind::File => "file",
    }
}

fn module_href(current: &ModulePath, target: &ModulePath, label: Option<&str>) -> String {
    let mut href = "../".repeat(current.segments().len());
    for segment in target.segments() {
        href.push_str(&url_path_segment(segment));
        href.push('/');
    }
    if href.is_empty() {
        href.push_str("./");
    }
    if let Some(label) = label {
        href.push('#');
        href.extend(utf8_percent_encode(label, NON_ALPHANUMERIC));
    }
    href
}

/// Relative URL of a resource file copied into the target module's page
/// directory; the real file name is percent-encoded as one extra segment.
fn resource_href(current: &ModulePath, target: &ModulePath, name: &str) -> String {
    let mut href = module_href(current, target, None);
    href.extend(utf8_percent_encode(name, URL_PATH_SEGMENT_ENCODE_SET));
    href
}

fn url_path_segment(segment: &str) -> String {
    utf8_percent_encode(&filesystem_segment(segment), URL_PATH_SEGMENT_ENCODE_SET).to_string()
}

fn filesystem_segment(segment: &str) -> String {
    use std::fmt::Write as _;

    let mut output = String::new();
    for character in segment.chars() {
        if character.is_control()
            || matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '~' | '.'
            )
        {
            for byte in character.to_string().bytes() {
                write!(output, "~{byte:02X}").unwrap();
            }
        } else {
            output.push(character);
        }
    }
    if is_windows_reserved_name(&output) {
        output.insert_str(0, "~00");
    }
    output
}

fn is_windows_reserved_name(segment: &str) -> bool {
    let upper = segment.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || upper
            .strip_prefix("COM")
            .or_else(|| upper.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
}

fn escape_html(output: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            _ => output.push(character),
        }
    }
}

fn escape_attribute(output: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            _ => output.push(character),
        }
    }
}

fn diagnostic_code(kind: &DiagnosticKind) -> &'static str {
    match kind {
        DiagnosticKind::DuplicateModule => "duplicate-module",
        DiagnosticKind::DuplicateLabel => "duplicate-label",
        DiagnosticKind::InvalidSyntax => "invalid-syntax",
        DiagnosticKind::UnresolvedModule => "unresolved-module",
        DiagnosticKind::UnresolvedLabel => "unresolved-label",
        DiagnosticKind::AmbiguousLabel => "ambiguous-label",
        DiagnosticKind::UnknownFunction => "unknown-function",
        DiagnosticKind::DuplicateFunction => "duplicate-function",
        DiagnosticKind::UnresolvedName => "unresolved-name",
        DiagnosticKind::InvalidFunction => "invalid-function",
        DiagnosticKind::InvalidArguments => "invalid-arguments",
        DiagnosticKind::TypeMismatch => "type-mismatch",
        DiagnosticKind::Evaluation => "evaluation",
        DiagnosticKind::ExternalReferenceUnsupported => "external-reference-unsupported",
        DiagnosticKind::ImportCycle => "import-cycle",
    }
}

fn completion_kind(kind: CompletionKind) -> &'static str {
    match kind {
        CompletionKind::Module => "module",
        CompletionKind::Function => "function",
        CompletionKind::Parameter => "parameter",
        CompletionKind::Attribute => "attribute",
    }
}

fn parse_absolute_module_path(value: &str) -> Option<ModulePath> {
    if value == "vault" {
        return Some(ModulePath::root());
    }
    let tail = value.strip_prefix("vault::")?;
    let segments = tail
        .split("::")
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    (!segments.is_empty()).then(|| ModulePath::from_segments(segments))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn embedded_and_remote_surfaces_share_serializable_core_requests() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        let path = root.path().join("README.not");
        fs::write(&path, "#heading[Title]@title").unwrap();
        let service = NotistService::new();
        let opened = service
            .execute(CoreRequest::OpenView {
                root: root.path().to_path_buf(),
                kind: ProtocolViewKind::Disk,
            })
            .unwrap();
        let CoreResponse::Opened { view_id, .. } = opened.response else {
            panic!("expected opened view")
        };
        let reply = service
            .execute(CoreRequest::WorkspaceSymbols {
                view_id,
                query: "title".into(),
            })
            .unwrap();
        let encoded = serde_json::to_string(&reply).unwrap();
        let CoreResponse::WorkspaceSymbols(symbols) = reply.response else {
            panic!("expected workspace symbols")
        };
        assert_eq!(symbols[0].kind, "annotation");
        assert!(encoded.contains("daemon_instance"));
        assert!(encoded.contains("revision"));
    }

    #[test]
    fn protocol_configuration_projects_plugin_schema_without_running_plugin_code() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        let path = root.path().join("README.not");
        fs::write(&path, "disk").unwrap();
        let path = dunce::canonicalize(path).unwrap();
        let service = NotistService::new();
        let opened = service
            .execute(CoreRequest::OpenView {
                root: root.path().to_path_buf(),
                kind: ProtocolViewKind::Session,
            })
            .unwrap();
        let CoreResponse::Opened { view_id, .. } = opened.response else {
            panic!("expected open view")
        };
        service
            .execute(CoreRequest::UpdateView {
                view_id,
                documents: vec![OverlayDocument {
                    path,
                    version: 1,
                    text: "#plugin::note()".into(),
                }],
                configuration: Some(ConfigurationRecord {
                    manifest_override: Some("configured = true".into()),
                    signatures: vec![NamedSignatureRecord {
                        name: "plugin::note".into(),
                        parameters: Vec::new(),
                        trailing_content: None,
                        result: TypeRecord::Content,
                    }],
                }),
            })
            .unwrap();
        let diagnostics = service
            .execute(CoreRequest::Diagnostics { view_id })
            .unwrap();
        let CoreResponse::Diagnostics(diagnostics) = diagnostics.response else {
            panic!("expected diagnostics")
        };
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn update_view_delta_upserts_and_removes_without_touching_other_overlays() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        let path = root.path().join("README.not");
        fs::write(&path, "disk").unwrap();
        let readme = dunce::canonicalize(&path).unwrap();
        let other = root.path().join("other.not");
        fs::write(&other, "other").unwrap();
        let other = dunce::canonicalize(other).unwrap();
        let service = NotistService::new();
        let opened = service
            .execute(CoreRequest::OpenView {
                root: root.path().to_path_buf(),
                kind: ProtocolViewKind::Session,
            })
            .unwrap();
        let CoreResponse::Opened { view_id, .. } = opened.response else {
            panic!("expected open view")
        };

        // Two full uploads establish two overlays.
        for (path, text) in [(&readme, "first"), (&other, "second")] {
            service
                .execute(CoreRequest::UpdateView {
                    view_id,
                    documents: vec![OverlayDocument {
                        path: path.clone(),
                        version: 1,
                        text: text.into(),
                    }],
                    configuration: None,
                })
                .unwrap();
        }

        // The delta changes only `README.not`; `other.not` keeps its overlay,
        // which a naive replace-with-subset would have dropped.
        let updated = service
            .execute(CoreRequest::UpdateViewDelta {
                view_id,
                upsert: vec![OverlayDocument {
                    path: readme.clone(),
                    version: 2,
                    text: "changed".into(),
                }],
                remove: Vec::new(),
            })
            .unwrap();

        let sources = service
            .execute(CoreRequest::Sources { view_id })
            .unwrap();
        let CoreResponse::Sources(sources) = sources.response else {
            panic!("expected sources")
        };
        let text_of = |path: &Path| {
            sources
                .iter()
                .find(|source| source.path == path)
                .map(|source| source.text.to_string())
                .unwrap_or_else(|| "<missing>".into())
        };
        assert_eq!(text_of(&readme), "changed");
        assert_eq!(text_of(&other), "second");
        assert_eq!(
            sources
                .iter()
                .find(|source| source.path == readme)
                .and_then(|source| source.document_version),
            Some(2),
            "the delta carries the new document version"
        );

        // Removing an overlay falls back to the disk content.
        service
            .execute(CoreRequest::UpdateViewDelta {
                view_id,
                upsert: Vec::new(),
                remove: vec![readme.clone()],
            })
            .unwrap();
        let sources = service
            .execute(CoreRequest::Sources { view_id })
            .unwrap();
        let CoreResponse::Sources(sources) = sources.response else {
            panic!("expected sources")
        };
        let text_of = |path: &Path| {
            sources
                .iter()
                .find(|source| source.path == path)
                .map(|source| source.text.to_string())
        };
        assert_eq!(text_of(&readme).as_deref(), Some("disk"));
        assert_eq!(text_of(&other).as_deref(), Some("second"));
        assert!(matches!(updated.response, CoreResponse::Updated));
    }

    #[test]
    fn cancelled_core_requests_stop_before_snapshot_work() {
        let service = NotistService::new();
        let cancelled = AtomicBool::new(true);
        let error = service
            .execute_cancellable(
                CoreRequest::Search {
                    view_id: ServiceViewId(999),
                    query: "anything".into(),
                },
                &cancelled,
            )
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    }

    #[test]
    fn disk_watcher_reloads_external_plugin_packages() {
        let base = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
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
                        "module": "semantic.wasm"
                    }}
                }}"#
            )
        };
        fs::write(package.join("plugin.json"), manifest("0.1.0")).unwrap();
        fs::write(
            root.join("Notist.toml"),
            "[plugins.demo]\npath = \"../pkg\"\n",
        )
        .unwrap();
        fs::write(root.join("README.not"), "#demo::echo(message: \"x\")[Hi]").unwrap();

        let service = NotistService::new();
        let opened = service
            .execute(CoreRequest::OpenView {
                root: root.clone(),
                kind: ProtocolViewKind::Disk,
            })
            .unwrap();
        let CoreResponse::Opened { view_id, .. } = opened.response else {
            panic!("expected opened view")
        };
        let initial = service
            .execute(CoreRequest::SnapshotSummary { view_id })
            .unwrap();
        let initial_revision = initial.snapshot.revision;

        fs::write(package.join("plugin.json"), manifest("0.2.0")).unwrap();
        let mut observed = initial_revision;
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(100));
            let reply = service
                .execute(CoreRequest::SnapshotSummary { view_id })
                .unwrap();
            observed = reply.snapshot.revision;
            if observed != initial_revision {
                break;
            }
        }
        assert_ne!(
            observed, initial_revision,
            "external plugin package changes should reload the disk snapshot"
        );
    }

    #[test]
    fn renders_manifest_web_component_contributions() {
        let base = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
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
                "package": "card",
                "version": "0.1.0",
                "api-version": "0.1",
                "render": {
                    "html": {
                        "contributions": [{
                            "element": "note",
                            "trusted": true,
                            "web-component": {
                                "tag": "notist-card",
                                "module": "assets/card.js",
                                "style": "assets/card.css"
                            }
                        }]
                    }
                },
                "wasm": {
                    "module": "semantic.wasm"
                }
            }"#,
        )
        .unwrap();
        fs::write(
            root.join("Notist.toml"),
            "[plugins.card]\npath = \"../pkg\"\n",
        )
        .unwrap();
        fs::write(
            root.join("README.not"),
            "#card::note(message: \"Hello\")[body]",
        )
        .unwrap();

        let service = NotistService::new();
        let opened = service
            .execute(CoreRequest::OpenView {
                root,
                kind: ProtocolViewKind::Disk,
            })
            .unwrap();
        let CoreResponse::Opened { view_id, .. } = opened.response else {
            panic!("expected opened view")
        };
        let reply = service
            .execute(CoreRequest::RenderWorkspace { view_id })
            .unwrap();
        let CoreResponse::RenderedWorkspace(rendered) = reply.response else {
            panic!("expected rendered workspace")
        };
        let home = rendered
            .pages
            .iter()
            .find(|page| page.module_segments.is_empty())
            .expect("home page");
        // Data-only declaration: the call stays a self-named leaf and the
        // web-component renderer projects it from name + fields.
        assert!(
            home.fragment.contains("<notist-card"),
            "fragment: {}",
            home.fragment
        );
        assert!(home.fragment.contains("data-message=\"Hello\""));
    }

    #[test]
    fn renders_heading_text_labels_and_virtual_module_titles() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::write(root.path().join("Notist.toml"), "").unwrap();
        fs::write(
            root.path().join("README.not"),
            "= 首页\n\nSee #<guide/简介> and #<guide/不存在>.",
        )
        .unwrap();
        fs::write(root.path().join("guide.not"), "= 指南\n\n== 简介\n\n内容").unwrap();
        fs::create_dir(root.path().join("notes")).unwrap();
        fs::write(root.path().join("notes/chapter.not"), "= 第一章").unwrap();
        let service = NotistService::new();
        let opened = service
            .execute(CoreRequest::OpenView {
                root: root.path().to_path_buf(),
                kind: ProtocolViewKind::Disk,
            })
            .unwrap();
        let CoreResponse::Opened { view_id, .. } = opened.response else {
            panic!("expected opened view")
        };
        let reply = service
            .execute(CoreRequest::RenderWorkspace { view_id })
            .unwrap();
        let CoreResponse::RenderedWorkspace(rendered) = reply.response else {
            panic!("expected rendered workspace")
        };
        let page = |segments: &[&str]| {
            rendered
                .pages
                .iter()
                .find(|page| page.module_segments == segments)
                .expect("page exists")
        };

        let home = page(&[]);
        assert_eq!(home.title.as_deref(), Some("首页"));
        assert_eq!(
            home.source.as_deref(),
            Some("= 首页\n\nSee #<guide/简介> and #<guide/不存在>.")
        );
        assert_eq!(
            home.headings,
            vec![RenderedHeadingRecord {
                level: 1,
                id: "首页".into(),
                text: "首页".into(),
            }]
        );
        // Heading-text labels resolve to the target module's anchor.
        assert!(
            home.fragment.contains("href=\"guide/#%E7%AE%80%E4%BB%8B\""),
            "{}",
            home.fragment
        );
        // Unknown labels stay visible but unclickable.
        assert!(home.fragment.contains("notist-reference-unresolved"));
        assert!(home.fragment.contains("guide/不存在"));
        assert!(!home.fragment.contains("#%E4%B8%8D%E5%AD%98%E5%9C%A8"));

        let guide = page(&["guide"]);
        assert_eq!(guide.title.as_deref(), Some("指南"));
        assert!(guide.fragment.contains("id=\"简介\""));
        assert!(
            guide.headings.iter().any(|heading| heading.level == 2
                && heading.id == "简介"
                && heading.text == "简介")
        );

        // The virtual module index lists child modules by their semantic title.
        let notes = page(&["notes"]);
        assert_eq!(notes.title, None);
        assert_eq!(notes.source, None);
        assert!(notes.headings.is_empty());
        assert!(notes.fragment.contains(">第一章</a>"), "{}", notes.fragment);
    }

    #[test]
    fn renders_resource_links_and_collects_resource_records() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::write(root.path().join("Notist.toml"), "").unwrap();
        fs::write(
            root.path().join("README.not"),
            "= Home\n\nLogo: #<vault::images/logo.png>, spec: #<vault::images/spec sheet.pdf>.",
        )
        .unwrap();
        fs::create_dir(root.path().join("images")).unwrap();
        fs::write(root.path().join("images/logo.png"), [0x89, 0x50]).unwrap();
        fs::write(root.path().join("images/spec sheet.pdf"), b"pdf").unwrap();
        let service = NotistService::new();
        let opened = service
            .execute(CoreRequest::OpenView {
                root: root.path().to_path_buf(),
                kind: ProtocolViewKind::Disk,
            })
            .unwrap();
        let CoreResponse::Opened { view_id, .. } = opened.response else {
            panic!("expected opened view")
        };
        let reply = service
            .execute(CoreRequest::RenderWorkspace { view_id })
            .unwrap();
        let CoreResponse::RenderedWorkspace(rendered) = reply.response else {
            panic!("expected rendered workspace")
        };
        let page = |segments: &[&str]| {
            rendered
                .pages
                .iter()
                .find(|page| page.module_segments == segments)
                .expect("page exists")
        };

        // References to resource files become relative links to the copied file.
        let home = page(&[]);
        assert!(
            home.fragment.contains("href=\"images/logo.png\""),
            "{}",
            home.fragment
        );
        assert!(
            home.fragment.contains("href=\"images/spec%20sheet.pdf\""),
            "{}",
            home.fragment
        );

        // The record carries every resource for the build layer to copy.
        assert_eq!(rendered.resources.len(), 2);
        let logo = rendered
            .resources
            .iter()
            .find(|resource| resource.name == "logo.png")
            .expect("logo resource recorded");
        assert_eq!(logo.module_segments, ["images"]);
        assert_eq!(logo.kind, "image");
        assert!(logo.source_path.ends_with("logo.png"));
        let spec = rendered
            .resources
            .iter()
            .find(|resource| resource.name == "spec sheet.pdf")
            .expect("spec resource recorded");
        assert_eq!(spec.kind, "file");

        // The virtual module index lists the resources after the child modules.
        let images = page(&["images"]);
        assert!(
            images
                .fragment
                .contains("<ul class=\"module-index module-resource-index\">"),
            "{}",
            images.fragment
        );
        assert!(
            images
                .fragment
                .contains("<a href=\"logo.png\" data-notist-kind=\"image\">logo.png</a>"),
            "{}",
            images.fragment
        );
        assert!(
            images
                .fragment
                .contains("href=\"spec%20sheet.pdf\" data-notist-kind=\"file\""),
            "{}",
            images.fragment
        );
    }

    #[test]
    fn queries_are_complete_ranked_and_grouped() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::write(root.path().join("Notist.toml"), "").unwrap();
        fs::write(
            root.path().join("README.not"),
            "= Searchable Workspace\n\nworkspace snapshot searchable\n\nworkspace snapshot second\n\n// secretcomment",
        )
        .unwrap();
        fs::write(
            root.path().join("long.not"),
            (1..=300)
                .map(|line| format!("line {line}\n"))
                .collect::<String>(),
        )
        .unwrap();
        let service = NotistService::new();
        let opened = service
            .execute(CoreRequest::OpenView {
                root: root.path().to_path_buf(),
                kind: ProtocolViewKind::Disk,
            })
            .unwrap();
        let CoreResponse::Opened { view_id, .. } = opened.response else {
            panic!("expected opened view")
        };
        let background = service
            .execute(CoreRequest::IndexRebuild {
                view_id,
                wait: false,
            })
            .unwrap();
        let CoreResponse::IndexStatus(background) = background.response else {
            panic!("expected background index status")
        };
        assert_eq!(background.health, "building");
        assert!(background.operation_handle.is_some());
        let completed = service
            .execute(CoreRequest::IndexRebuild {
                view_id,
                wait: true,
            })
            .unwrap();
        let CoreResponse::IndexStatus(completed) = completed.response else {
            panic!("expected completed index status")
        };
        assert_eq!(completed.health, "ready");
        assert_eq!(completed.operation_handle, background.operation_handle);
        let query = crate::query::SearchQuery {
            query: "workspace snapshot".into(),
            mode: crate::query::SearchMode::Lexical,
            scopes: Vec::new(),
            fields: crate::query::SearchField::defaults(),
            operator: crate::query::SearchOperator::All,
            group_by: Some(crate::query::SearchGroup::Match),
            ignore_case: false,
            fuzzy_distance: 1,
            wait_index_ms: 2000,
            snippet_bytes: 128,
        };
        let first = service
            .execute(CoreRequest::SearchPage {
                view_id,
                query: query.clone(),
            })
            .unwrap();
        let CoreResponse::SearchPage(first) = first.response else {
            panic!("expected search page")
        };
        assert_eq!(first.records.len(), 2);
        assert!(first.records[0].score.is_some());
        assert!(first.records[0].match_range.is_some());
        assert!(first.records[0].excerpt.to_lowercase().contains("workspace"));

        let mut grouped_query = query.clone();
        grouped_query.group_by = None;
        let grouped = service
            .execute(CoreRequest::SearchPage {
                view_id,
                query: grouped_query,
            })
            .unwrap();
        let CoreResponse::SearchPage(grouped) = grouped.response else {
            panic!("expected grouped search page")
        };
        assert_eq!(grouped.records.len(), 1);
        let metadata = grouped.search.as_ref().unwrap();
        assert_eq!(metadata.group_by, crate::query::SearchGroup::Source);
        assert_eq!(metadata.ordering, "relevance");
        assert!(metadata.index_stamp.is_some());

        let fuzzy = service
            .execute(CoreRequest::SearchPage {
                view_id,
                query: crate::query::SearchQuery {
                    query: "serchable".into(),
                    mode: crate::query::SearchMode::Fuzzy,
                    fields: crate::query::SearchField::defaults(),
                    scopes: Vec::new(),
                    operator: crate::query::SearchOperator::All,
                    group_by: None,
                    ignore_case: false,
                    fuzzy_distance: 1,
                    wait_index_ms: 2000,
                    snippet_bytes: 128,
                },
            })
            .unwrap();
        let CoreResponse::SearchPage(fuzzy) = fuzzy.response else {
            panic!("expected fuzzy page")
        };
        assert!(!fuzzy.records.is_empty());

        for (field, expected) in [
            (crate::query::SearchField::Comment, true),
            (crate::query::SearchField::Body, false),
        ] {
            let result = service
                .execute(CoreRequest::SearchPage {
                    view_id,
                    query: crate::query::SearchQuery {
                        query: "secretcomment".into(),
                        mode: crate::query::SearchMode::Lexical,
                        fields: vec![field],
                        scopes: Vec::new(),
                        operator: crate::query::SearchOperator::All,
                        group_by: None,
                        ignore_case: false,
                        fuzzy_distance: 1,
                        wait_index_ms: 2000,
                        snippet_bytes: 128,
                    },
                })
                .unwrap();
            let CoreResponse::SearchPage(result) = result.response else {
                panic!("expected field search page")
            };
            assert_eq!(!result.records.is_empty(), expected);
        }

        fs::write(
            root.path().join("README.not"),
            "= Changed\n\nincrementalneedle",
        )
        .unwrap();
        service
            .execute(CoreRequest::ReloadDiskView { view_id })
            .unwrap();
        for (term, expected) in [("incrementalneedle", true), ("searchable", false)] {
            let result = service
                .execute(CoreRequest::SearchPage {
                    view_id,
                    query: crate::query::SearchQuery {
                        query: term.into(),
                        mode: crate::query::SearchMode::Lexical,
                        fields: crate::query::SearchField::defaults(),
                        scopes: Vec::new(),
                        operator: crate::query::SearchOperator::All,
                        group_by: None,
                        ignore_case: false,
                        fuzzy_distance: 1,
                        wait_index_ms: 2000,
                        snippet_bytes: 128,
                    },
                })
                .unwrap();
            let CoreResponse::SearchPage(result) = result.response else {
                panic!("expected incremental search page")
            };
            assert_eq!(!result.records.is_empty(), expected);
        }
    }

    const ANCESTORS_SOURCE: &str =
        "@![status = \"draft\"]\n\n@[wip]\n= 安装\n\n先读概述。\n\n== 故障排除\n\n出问题时看日志。\n\n= 后记\n\n完。\n";

    fn ancestors_fixture(
        file_name: &str,
        source: &str,
    ) -> (NotistService, ServiceViewId, tempfile::TempDir) {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::write(root.path().join(file_name), source).unwrap();
        let service = NotistService::new();
        let opened = service
            .execute(CoreRequest::OpenView {
                root: root.path().to_path_buf(),
                kind: ProtocolViewKind::Disk,
            })
            .unwrap();
        let CoreResponse::Opened { view_id, .. } = opened.response else {
            panic!("expected opened view")
        };
        (service, view_id, root)
    }

    fn ancestors_response(
        service: &NotistService,
        view_id: ServiceViewId,
        selector: &str,
        offset: Option<usize>,
        byte_range: Option<ByteRange>,
    ) -> CoreResponse {
        service
            .execute(CoreRequest::Ancestors {
                view_id,
                query: crate::query::AncestorsQuery {
                    selector: crate::query::Selector::parse(selector),
                    offset,
                    byte_range,
                },
            })
            .unwrap()
            .response
    }

    fn expect_ancestors(response: CoreResponse) -> Vec<crate::query::AncestorRecord> {
        match response {
            CoreResponse::Ancestors(result) => result.records,
            other => panic!("expected ancestors response, got {other:?}"),
        }
    }

    #[test]
    fn ancestors_module_selector_returns_the_full_tree_with_module_attributes_at_the_root() {
        let (service, view_id, _root) = ancestors_fixture("guide.not", ANCESTORS_SOURCE);
        let records = expect_ancestors(ancestors_response(
            &service, view_id, "vault::guide", None, None,
        ));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].kind, "module");
        assert_eq!(records[0].location.module, "vault::guide");
        assert_eq!(records[0].attributes.len(), 1);
        assert_eq!(
            records[0].attributes[0].properties,
            vec![("status".to_owned(), "draft".to_owned())]
        );
        let sections = records[0]
            .children
            .iter()
            .map(|child| child.name.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(sections, vec![Some("安装"), Some("后记")]);
    }

    #[test]
    fn ancestors_point_in_section_body_claims_heading_annotations_on_the_section() {
        let (service, view_id, _root) = ancestors_fixture("guide.not", ANCESTORS_SOURCE);
        let offset = ANCESTORS_SOURCE.find("概述").unwrap();
        let records = expect_ancestors(ancestors_response(
            &service,
            view_id,
            "vault::guide",
            Some(offset),
            None,
        ));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].kind, "module");
        assert_eq!(records[0].attributes[0].properties[0].0, "status");
        assert_eq!(records[0].children.len(), 1);
        let section = &records[0].children[0];
        assert_eq!(section.kind, "core::section");
        assert_eq!(section.level, Some(1));
        assert_eq!(section.name.as_deref(), Some("安装"));
        assert_eq!(section.attributes.len(), 1);
        assert_eq!(section.attributes[0].id.as_deref(), Some("wip"));
        assert_eq!(section.children.len(), 1);
        let paragraph = &section.children[0];
        assert_eq!(paragraph.kind, "core::paragraph");
        assert!(paragraph.attributes.is_empty());
        assert_eq!(paragraph.children[0].kind, "core::text");
    }

    #[test]
    fn ancestors_point_inside_a_heading_claims_on_the_heading_node() {
        let (service, view_id, _root) = ancestors_fixture("guide.not", ANCESTORS_SOURCE);
        let offset = ANCESTORS_SOURCE.find("安装").unwrap();
        let records = expect_ancestors(ancestors_response(
            &service,
            view_id,
            "vault::guide",
            Some(offset),
            None,
        ));
        assert_eq!(records.len(), 1);
        let section = &records[0].children[0];
        assert!(section.attributes.is_empty());
        assert_eq!(section.children.len(), 1);
        let heading = &section.children[0];
        assert_eq!(heading.kind, "core::heading");
        assert_eq!(heading.name.as_deref(), Some("安装"));
        assert_eq!(heading.level, Some(1));
        assert_eq!(heading.attributes[0].id.as_deref(), Some("wip"));
        assert_eq!(heading.children[0].kind, "core::text");
    }

    #[test]
    fn ancestors_region_inside_a_nested_section_reports_every_ancestor_level() {
        let (service, view_id, _root) = ancestors_fixture("guide.not", ANCESTORS_SOURCE);
        let start = ANCESTORS_SOURCE.find("出问题").unwrap();
        let end = ANCESTORS_SOURCE.find("看日志").unwrap() + "看日志".len();
        let records = expect_ancestors(ancestors_response(
            &service,
            view_id,
            "vault::guide",
            None,
            Some(ByteRange { start, end }),
        ));
        assert_eq!(records.len(), 1);
        let outer = &records[0].children[0];
        assert_eq!(outer.name.as_deref(), Some("安装"));
        assert_eq!(outer.attributes[0].id.as_deref(), Some("wip"));
        assert_eq!(outer.children.len(), 1);
        let inner = &outer.children[0];
        assert_eq!(inner.name.as_deref(), Some("故障排除"));
        assert_eq!(inner.level, Some(2));
        assert!(inner.attributes.is_empty());
        assert_eq!(inner.children[0].kind, "core::paragraph");
        assert_eq!(inner.children[0].children[0].kind, "core::text");
    }

    #[test]
    fn ancestors_item_name_selector_starts_at_the_named_heading() {
        let (service, view_id, _root) = ancestors_fixture("guide.not", ANCESTORS_SOURCE);
        let records = expect_ancestors(ancestors_response(
            &service,
            view_id,
            "vault::guide/安装",
            None,
            None,
        ));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].kind, "module");
        let section = &records[0].children[0];
        assert_eq!(section.kind, "core::section");
        let heading = &section.children[0];
        assert_eq!(heading.kind, "core::heading");
        assert_eq!(heading.name.as_deref(), Some("安装"));
        assert_eq!(heading.attributes[0].id.as_deref(), Some("wip"));
    }

    #[test]
    fn ancestors_region_across_sibling_sections_reports_every_grazed_scope() {
        let (service, view_id, _root) = ancestors_fixture("guide.not", ANCESTORS_SOURCE);
        let start = ANCESTORS_SOURCE.find("出问题").unwrap();
        let end = ANCESTORS_SOURCE.find("完。").unwrap() + "完。".len();
        let records = expect_ancestors(ancestors_response(
            &service,
            view_id,
            "vault::guide",
            None,
            Some(ByteRange { start, end }),
        ));
        // The region grazes the tail of the nested 故障排除 scope and the head
        // of the sibling 后记 scope: three scopes, each with its path to the
        // module root.
        let names = records[0]
            .children
            .iter()
            .map(|child| child.name.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(names, vec![Some("安装"), Some("后记")]);
        let nested = &records[0].children[0];
        assert_eq!(nested.attributes[0].id.as_deref(), Some("wip"));
        assert_eq!(nested.children.len(), 1);
        let inner = &nested.children[0];
        assert_eq!(inner.name.as_deref(), Some("故障排除"));
        assert_eq!(inner.children[0].kind, "core::paragraph");
        let sibling = &records[0].children[1];
        assert!(sibling.attributes.is_empty());
        assert_eq!(sibling.children[0].name.as_deref(), Some("后记"));
        assert_eq!(sibling.children[1].kind, "core::paragraph");
    }

    #[test]
    fn ancestors_ambiguous_item_names_are_a_typed_error() {
        let (service, view_id, _root) = ancestors_fixture(
            "dupe.not",
            "= 重复\n\n一段。\n\n== 重复\n\n另一段。\n",
        );
        let response = ancestors_response(&service, view_id, "vault::dupe/重复", None, None);
        let CoreResponse::QueryError(error) = response else {
            panic!("expected a query error")
        };
        assert_eq!(error.code, "ambiguous_selector");
    }

    #[test]
    fn ancestors_rejects_offsets_that_are_not_utf8_boundaries() {
        let (service, view_id, _root) = ancestors_fixture("guide.not", ANCESTORS_SOURCE);
        let offset = ANCESTORS_SOURCE.find("概述").unwrap() + 1;
        let response = ancestors_response(
            &service,
            view_id,
            "vault::guide",
            Some(offset),
            None,
        );
        let CoreResponse::QueryError(error) = response else {
            panic!("expected a query error")
        };
        assert_eq!(error.code, "invalid_argument");
    }

    #[test]
    fn ancestors_rejects_offset_and_byte_range_together() {
        let (service, view_id, _root) = ancestors_fixture("guide.not", ANCESTORS_SOURCE);
        let response = ancestors_response(
            &service,
            view_id,
            "vault::guide",
            Some(0),
            Some(ByteRange { start: 0, end: 4 }),
        );
        let CoreResponse::QueryError(error) = response else {
            panic!("expected a query error")
        };
        assert_eq!(error.code, "invalid_argument");
    }

    fn items_records(
        service: &NotistService,
        view_id: ServiceViewId,
        selector: &str,
    ) -> Result<Vec<crate::query::ItemRecord>, crate::query::ToolError> {
        match service
            .execute(CoreRequest::Items {
                view_id,
                query: crate::query::ItemsQuery {
                    selector: crate::query::Selector::parse(selector),
                },
            })
            .unwrap()
            .response
        {
            CoreResponse::Items(result) => Ok(result.records),
            CoreResponse::QueryError(error) => Err(error),
            other => panic!("expected items response, got {other:?}"),
        }
    }

    #[test]
    fn items_override_ambiguity_and_name_selectors() {
        let (service, view_id, _root) = ancestors_fixture(
            "mixed.not",
            "#heading[原标题]@renamed\n\n正文。\n\n= 原标题\n\n另一节。\n",
        );
        let records = items_records(&service, view_id, "vault::mixed").unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].name, "renamed");
        assert_eq!(records[0].origin, "id");
        assert!(!records[0].ambiguous);
        assert_eq!(records[0].attributes.len(), 1);
        assert_eq!(records[0].attributes[0].id.as_deref(), Some("renamed"));
        assert_eq!(records[1].name, "原标题");
        assert_eq!(records[1].origin, "heading");
        assert!(records[1].ambiguous);

        let error = items_records(&service, view_id, "vault::mixed/renamed").unwrap_err();
        assert_eq!(error.code, "invalid_argument");
    }

    #[test]
    fn items_lists_resources_for_virtual_modules() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::write(root.path().join("readme.not"), "hello\n").unwrap();
        fs::create_dir_all(root.path().join("assets")).unwrap();
        fs::write(root.path().join("assets/logo.png"), "png\n").unwrap();
        let service = NotistService::new();
        let opened = service
            .execute(CoreRequest::OpenView {
                root: root.path().to_path_buf(),
                kind: ProtocolViewKind::Disk,
            })
            .unwrap();
        let CoreResponse::Opened { view_id, .. } = opened.response else {
            panic!("expected opened view")
        };
        let records = items_records(&service, view_id, "vault::assets").unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "logo.png");
        assert_eq!(records[0].kind, "resource:image");
        assert_eq!(records[0].origin, "resource");
        assert_eq!(
            records[0].location.relative_path,
            std::path::PathBuf::from("assets/logo.png")
        );
    }
}
