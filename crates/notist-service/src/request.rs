use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use notist_analysis::{
    AnalyzerConfiguration, CompletionKind, DiagnosticKind, DocumentVersions, SignatureSet,
    SourceOverlays, WorkspaceSymbolKind,
};
use notist_html::{RenderOptions, render_with_resolvers};
use notist_model::{DefaultValue, FunctionSignature, ModulePath, Parameter, TextRange, Type};
use percent_encoding::{AsciiSet, CONTROLS, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};

use crate::{NotistService, ServiceViewId, SnapshotIdentity, VaultIdentity, ViewKind};

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
    SnapshotSummary {
        view_id: ServiceViewId,
    },
    Sources {
        view_id: ServiceViewId,
    },
    ReloadDiskView {
        view_id: ServiceViewId,
    },
    Diagnostics {
        view_id: ServiceViewId,
    },
    FingerprintSource {
        view_id: ServiceViewId,
        path: PathBuf,
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
    Outline {
        view_id: ServiceViewId,
    },
    WorkspaceSymbols {
        view_id: ServiceViewId,
        query: String,
    },
    Search {
        view_id: ServiceViewId,
        query: String,
    },
    RenderWorkspace {
        view_id: ServiceViewId,
    },
    ProposeEdit {
        view_id: ServiceViewId,
        base_revision: u64,
        operations: Vec<EditOperation>,
    },
    ApplyEdit {
        view_id: ServiceViewId,
        plan_hash: String,
        expected_fingerprints: Vec<SourceFingerprint>,
        idempotency_key: String,
    },
    RenameSource {
        view_id: ServiceViewId,
        from: PathBuf,
        to: PathBuf,
        expected_fingerprint: String,
        idempotency_key: String,
    },
}

impl CoreRequest {
    pub fn view_id(&self) -> Option<ServiceViewId> {
        match self {
            Self::OpenView { .. } => None,
            Self::CloseView { view_id }
            | Self::UpdateView { view_id, .. }
            | Self::SnapshotSummary { view_id }
            | Self::Sources { view_id }
            | Self::ReloadDiskView { view_id }
            | Self::Diagnostics { view_id }
            | Self::FingerprintSource { view_id, .. }
            | Self::Inspect { view_id }
            | Self::Definition { view_id, .. }
            | Self::References { view_id, .. }
            | Self::ReferencesTo { view_id, .. }
            | Self::Completion { view_id, .. }
            | Self::Hover { view_id, .. }
            | Self::DocumentSymbols { view_id, .. }
            | Self::Outline { view_id }
            | Self::WorkspaceSymbols { view_id, .. }
            | Self::Search { view_id, .. }
            | Self::RenderWorkspace { view_id }
            | Self::ProposeEdit { view_id, .. }
            | Self::ApplyEdit { view_id, .. }
            | Self::RenameSource { view_id, .. } => Some(*view_id),
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
    Array(Box<TypeRecord>),
    Dict(Box<TypeRecord>, Box<TypeRecord>),
    Function,
    Optional(Box<TypeRecord>),
    Union(Vec<TypeRecord>),
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
            TypeRecord::Array(item) => Self::Array(Box::new((*item).into())),
            TypeRecord::Dict(key, value) => {
                Self::Dict(Box::new((*key).into()), Box::new((*value).into()))
            }
            TypeRecord::Function => Self::Function,
            TypeRecord::Optional(inner) => Self::Optional(Box::new((*inner).into())),
            TypeRecord::Union(members) => {
                Self::Union(members.into_iter().map(Into::into).collect())
            }
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
    Sources(Vec<SourceRecord>),
    Reloaded,
    Diagnostics(Vec<DiagnosticRecord>),
    SourceFingerprint(Option<SourceFingerprint>),
    Inspect(InspectRecord),
    Definition(Option<LocationRecord>),
    References(Vec<LocationRecord>),
    Completion(Vec<CompletionRecord>),
    Hover(Option<HoverRecord>),
    DocumentSymbols(Vec<DocumentSymbolRecord>),
    Outline(Vec<OutlineRecord>),
    WorkspaceSymbols(Vec<WorkspaceSymbolRecord>),
    Search(Vec<SearchRecord>),
    RenderedWorkspace(RenderedWorkspaceRecord),
    EditPlan(EditPlanRecord),
    EditApplied(ApplyEditRecord),
    SourceRenamed(RenameSourceRecord),
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagnosticRecord {
    pub path: Option<PathBuf>,
    pub source: Option<String>,
    pub range: Option<ByteRange>,
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InspectRecord {
    pub modules: Vec<ModuleRecord>,
    pub references: Vec<ReferenceRecord>,
    pub semantic_items: Vec<SemanticItemRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SemanticItemRecord {
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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReferenceRecord {
    pub source_module: String,
    pub range: ByteRange,
    pub target_module: String,
    pub target_label: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocationRecord {
    pub path: PathBuf,
    pub source: String,
    pub range: ByteRange,
    pub is_definition: bool,
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
pub struct OutlineRecord {
    pub path: PathBuf,
    pub symbols: Vec<DocumentSymbolRecord>,
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
    pub evaluation_diagnostics: Vec<DiagnosticRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenderedPageRecord {
    pub module_segments: Vec<String>,
    pub fragment: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EditOperation {
    pub path: PathBuf,
    pub range: ByteRange,
    pub replacement: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceFingerprint {
    pub path: PathBuf,
    pub fingerprint: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EditPlanRecord {
    pub plan_hash: String,
    pub base_revision: u64,
    pub affected_sources: Vec<SourceFingerprint>,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ApplyEditRecord {
    pub plan_hash: String,
    pub idempotency_key: String,
    pub resulting_fingerprints: Vec<SourceFingerprint>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RenameSourceRecord {
    pub from: PathBuf,
    pub to: PathBuf,
    pub idempotency_key: String,
}

#[derive(Clone)]
pub(crate) struct StoredEditPlan {
    pub view_id: ServiceViewId,
    pub vault: VaultIdentity,
    pub operations: Vec<EditOperation>,
    pub fingerprints: Vec<SourceFingerprint>,
    pub diagnostics: Vec<String>,
}

impl NotistService {
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
                            message: diagnostic.message.clone(),
                        })
                        .collect()
                })?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::Diagnostics(diagnostics),
                })
            }
            CoreRequest::FingerprintSource { view_id, path } => {
                let (snapshot, fingerprint) = self.with_snapshot(view_id, |workspace| {
                    let file_id = workspace.file_id(&path)?;
                    let source = workspace.source(file_id)?;
                    Some(source_fingerprint(path, &source.text))
                })?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::SourceFingerprint(fingerprint),
                })
            }
            CoreRequest::Inspect { view_id } => {
                let (snapshot, inspect) =
                    self.with_snapshot(view_id, |workspace| InspectRecord {
                        modules: workspace
                            .modules()
                            .map(|module| ModuleRecord {
                                logical_path: module.logical_path.to_string(),
                                source_path: module.source_path.clone(),
                                virtual_module: module.source_path.is_none(),
                            })
                            .collect(),
                        references: workspace
                            .references()
                            .iter()
                            .map(|reference| ReferenceRecord {
                                source_module: reference.source_module.to_string(),
                                range: reference.range.into(),
                                target_module: reference.target_module.to_string(),
                                target_label: reference.target_label.clone(),
                            })
                            .collect(),
                        semantic_items: workspace
                            .modules()
                            .flat_map(|module| {
                                let module_name = module.logical_path.to_string();
                                let mut items = Vec::new();
                                if let Some(parse) = &module.parse {
                                    items.extend(parse.annotations().into_iter().map(
                                        |annotation| {
                                            SemanticItemRecord {
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
                                    items.extend(parse.calls().into_iter().map(|call| {
                                        let (range, kind) = match call.trailing.first() {
                                            Some(body) => (body.payload_range, "content call"),
                                            None => (call.range, "call"),
                                        };
                                        SemanticItemRecord {
                                            module: module_name.clone(),
                                            range: range.into(),
                                            kind: kind.into(),
                                            name: Some(call.name.value.clone()),
                                        }
                                    }));
                                    items.extend(parse.raw_literals().into_iter().map(|raw| {
                                        let kind = match raw.form {
                                            notist_syntax::RawLiteralForm::Inline => "inline raw",
                                            notist_syntax::RawLiteralForm::Fenced => "fenced raw",
                                        };
                                        SemanticItemRecord {
                                            module: module_name.clone(),
                                            range: raw.payload_range.into(),
                                            kind: kind.into(),
                                            name: raw.tag.as_ref().map(|tag| tag.value.clone()),
                                        }
                                    }));
                                }
                                items
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
            CoreRequest::Outline { view_id } => {
                let (snapshot, outline) = self.with_snapshot(view_id, |workspace| {
                    workspace
                        .sources()
                        .map(|source| OutlineRecord {
                            path: source.canonical_path.clone(),
                            symbols: workspace
                                .document_symbols(source.file_id)
                                .into_iter()
                                .map(|symbol| DocumentSymbolRecord {
                                    name: symbol.name,
                                    level: symbol.level,
                                    range: symbol.range.into(),
                                })
                                .collect(),
                        })
                        .filter(|outline| !outline.symbols.is_empty())
                        .collect()
                })?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::Outline(outline),
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
            CoreRequest::RenderWorkspace { view_id } => {
                let (snapshot, rendered) = self
                    .with_snapshot(view_id, |workspace| render_workspace(workspace, cancelled))?;
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::RenderedWorkspace(rendered?),
                })
            }
            CoreRequest::ProposeEdit {
                view_id,
                base_revision,
                operations,
            } => {
                let (snapshot, proposed) = self.with_snapshot(view_id, |workspace| {
                    if workspace.revision().raw() != base_revision {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!(
                                "base revision {base_revision} does not match current revision {}",
                                workspace.revision().raw()
                            ),
                        ));
                    }
                    let mut fingerprints = Vec::new();
                    let mut diagnostics = Vec::new();
                    for operation in &operations {
                        let Some(file_id) = workspace.file_id(&operation.path) else {
                            diagnostics.push(format!(
                                "source `{}` is not part of the captured view",
                                operation.path.display()
                            ));
                            continue;
                        };
                        let source = workspace.source(file_id).unwrap();
                        if operation.range.start > operation.range.end
                            || operation.range.end > source.text.len()
                            || !source.text.is_char_boundary(operation.range.start)
                            || !source.text.is_char_boundary(operation.range.end)
                        {
                            diagnostics.push(format!(
                                "edit range {}..{} is invalid for `{}`",
                                operation.range.start,
                                operation.range.end,
                                operation.path.display()
                            ));
                        }
                        if !fingerprints
                            .iter()
                            .any(|item: &SourceFingerprint| item.path == operation.path)
                        {
                            fingerprints
                                .push(source_fingerprint(operation.path.clone(), &source.text));
                        }
                    }
                    Ok((fingerprints, diagnostics))
                })?;
                let (fingerprints, diagnostics) = proposed?;
                let plan_hash = edit_plan_hash(
                    view_id,
                    base_revision,
                    &snapshot.vault,
                    &operations,
                    &fingerprints,
                )?;
                self.edit_plans.lock().unwrap().insert(
                    plan_hash.clone(),
                    StoredEditPlan {
                        view_id,
                        vault: snapshot.vault.clone(),
                        operations,
                        fingerprints: fingerprints.clone(),
                        diagnostics: diagnostics.clone(),
                    },
                );
                Ok(CoreReply {
                    snapshot,
                    response: CoreResponse::EditPlan(EditPlanRecord {
                        plan_hash,
                        base_revision,
                        affected_sources: fingerprints,
                        diagnostics,
                    }),
                })
            }
            CoreRequest::ApplyEdit {
                view_id,
                plan_hash,
                expected_fingerprints,
                idempotency_key,
            } => self.apply_edit(view_id, plan_hash, expected_fingerprints, idempotency_key),
            CoreRequest::RenameSource {
                view_id,
                from,
                to,
                expected_fingerprint,
                idempotency_key,
            } => self.rename_source(view_id, from, to, expected_fingerprint, idempotency_key),
        }
    }
}

impl NotistService {
    fn apply_edit(
        &self,
        view_id: ServiceViewId,
        plan_hash: String,
        expected_fingerprints: Vec<SourceFingerprint>,
        idempotency_key: String,
    ) -> io::Result<CoreReply> {
        if idempotency_key.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "idempotency key must not be empty",
            ));
        }
        if let Some(applied) = self
            .applied_edits
            .lock()
            .unwrap()
            .get(&idempotency_key)
            .cloned()
        {
            if applied.plan_hash != plan_hash {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "idempotency key was already used for another edit plan",
                ));
            }
            let (snapshot, ()) = self.with_snapshot(view_id, |_| ())?;
            return Ok(CoreReply {
                snapshot,
                response: CoreResponse::EditApplied(applied),
            });
        }
        let plan = self
            .edit_plans
            .lock()
            .unwrap()
            .get(&plan_hash)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown edit plan"))?;
        if plan.view_id != view_id || plan.fingerprints != expected_fingerprints {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "edit plan view or expected fingerprints do not match",
            ));
        }
        if !plan.diagnostics.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "edit plan contains validation diagnostics",
            ));
        }
        let (host, _, kind) = self.view(view_id)?;
        if kind != ViewKind::Disk {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "disk writes require a disk view",
            ));
        }
        if host.identity != plan.vault {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "edit plan belongs to another vault",
            ));
        }
        let _write = host.write_lock.lock().unwrap();
        let mut texts = BTreeMap::new();
        for expected in &expected_fingerprints {
            let text = std::fs::read_to_string(&expected.path)?;
            if source_fingerprint(expected.path.clone(), &text) != *expected {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "source `{}` changed after edit proposal",
                        expected.path.display()
                    ),
                ));
            }
            texts.insert(expected.path.clone(), text);
        }
        for (path, text) in &mut texts {
            let mut operations = plan
                .operations
                .iter()
                .filter(|operation| &operation.path == path)
                .collect::<Vec<_>>();
            operations.sort_by_key(|operation| std::cmp::Reverse(operation.range.start));
            let mut previous_start = text.len();
            for operation in operations {
                if operation.range.end > previous_start {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("overlapping edits for `{}`", path.display()),
                    ));
                }
                text.replace_range(
                    operation.range.start..operation.range.end,
                    &operation.replacement,
                );
                previous_start = operation.range.start;
            }
        }
        for (path, text) in &texts {
            replace_file(path, text.as_bytes(), &plan_hash)?;
        }
        let snapshot = host.disk.lock().unwrap().reload()?;
        let snapshot = self.snapshot_identity(view_id, &host, &snapshot);
        let resulting_fingerprints = texts
            .iter()
            .map(|(path, text)| source_fingerprint(path.clone(), text))
            .collect();
        let applied = ApplyEditRecord {
            plan_hash,
            idempotency_key: idempotency_key.clone(),
            resulting_fingerprints,
        };
        self.applied_edits
            .lock()
            .unwrap()
            .insert(idempotency_key, applied.clone());
        Ok(CoreReply {
            snapshot,
            response: CoreResponse::EditApplied(applied),
        })
    }

    fn rename_source(
        &self,
        view_id: ServiceViewId,
        from: PathBuf,
        to: PathBuf,
        expected_fingerprint: String,
        idempotency_key: String,
    ) -> io::Result<CoreReply> {
        if idempotency_key.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "idempotency key must not be empty",
            ));
        }
        if let Some(renamed) = self
            .renamed_sources
            .lock()
            .unwrap()
            .get(&idempotency_key)
            .cloned()
        {
            if renamed.from != from || renamed.to != to {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "idempotency key was already used for another rename",
                ));
            }
            let (snapshot, ()) = self.with_snapshot(view_id, |_| ())?;
            return Ok(CoreReply {
                snapshot,
                response: CoreResponse::SourceRenamed(renamed),
            });
        }
        let (host, _, kind) = self.view(view_id)?;
        if kind != ViewKind::Disk {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "source rename requires a disk view",
            ));
        }
        let from = dunce::canonicalize(from)?;
        let to_parent = dunce::canonicalize(
            to.parent()
                .ok_or_else(|| io::Error::other("rename target has no parent"))?,
        )?;
        let to = to_parent.join(
            to.file_name()
                .ok_or_else(|| io::Error::other("rename target has no file name"))?,
        );
        if !from.starts_with(&host.identity.canonical_root)
            || !to.starts_with(&host.identity.canonical_root)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "rename source and target must remain inside the vault",
            ));
        }
        if to.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "rename target already exists",
            ));
        }
        let _write = host.write_lock.lock().unwrap();
        let source = std::fs::read_to_string(&from)?;
        if source_fingerprint(from.clone(), &source).fingerprint != expected_fingerprint {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "rename source changed after it was selected",
            ));
        }
        std::fs::rename(&from, &to)?;
        host.engine.rename_source(&from, &to)?;
        let snapshot = host.disk.lock().unwrap().reload()?;
        let snapshot = self.snapshot_identity(view_id, &host, &snapshot);
        let renamed = RenameSourceRecord {
            from,
            to,
            idempotency_key: idempotency_key.clone(),
        };
        self.renamed_sources
            .lock()
            .unwrap()
            .insert(idempotency_key, renamed.clone());
        Ok(CoreReply {
            snapshot,
            response: CoreResponse::SourceRenamed(renamed),
        })
    }
}

fn source_fingerprint(path: PathBuf, text: &str) -> SourceFingerprint {
    SourceFingerprint {
        path,
        fingerprint: format!("{:016x}", super::fingerprint(text.as_bytes())),
    }
}

fn edit_plan_hash(
    view_id: ServiceViewId,
    revision: u64,
    vault: &VaultIdentity,
    operations: &[EditOperation],
    fingerprints: &[SourceFingerprint],
) -> io::Result<String> {
    let payload = serde_json::to_vec(&(view_id, revision, vault, operations, fingerprints))
        .map_err(io::Error::other)?;
    Ok(format!("{:016x}", super::fingerprint(&payload)))
}

fn replace_file(path: &Path, contents: &[u8], plan_hash: &str) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("edited source has no parent directory"))?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("edited source has no file name"))?
        .to_string_lossy();
    let temporary = parent.join(format!(".{name}.notist-{plan_hash}.tmp"));
    std::fs::write(&temporary, contents)?;
    replace_file_platform(&temporary, path)
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

fn render_workspace(
    workspace: &notist_analysis::WorkspaceSnapshot,
    cancelled: &AtomicBool,
) -> io::Result<RenderedWorkspaceRecord> {
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
    let mut pages = Vec::new();
    let mut evaluation_diagnostics = Vec::new();
    for module in &modules {
        if cancelled.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "request cancelled",
            ));
        }
        let fragment = if let (Some(source_path), Some(source)) =
            (&module.source_path, module.source.as_deref())
        {
            let structured = workspace
                .structured_module(module.id)
                .expect("source-backed modules have structured results");
            let current = &module.logical_path;
            let resolver = |target: &ModulePath, label: Option<&str>| {
                known
                    .contains(target)
                    .then(|| module_href(current, target, label))
            };
            let source_ids = module
                .parse
                .as_ref()
                .into_iter()
                .flat_map(|parse| parse.annotations())
                .filter_map(|annotation| {
                    annotation
                        .attributes
                        .id
                        .as_ref()
                        .map(|id| (annotation.scope_range, id.value.clone()))
                })
                .collect::<Vec<_>>();
            let source_id_resolver = |range: TextRange| {
                source_ids
                    .iter()
                    .find(|(scope_range, _)| {
                        scope_range.start <= range.start && range.end <= scope_range.end
                    })
                    .map(|(_, id)| id.clone())
            };
            for diagnostic in structured.diagnostics {
                evaluation_diagnostics.push(DiagnosticRecord {
                    path: Some(source_path.clone()),
                    source: Some(source.to_owned()),
                    range: Some(diagnostic.range.into()),
                    code: "evaluation".into(),
                    message: diagnostic.message,
                });
            }
            render_with_resolvers(
                &structured.document,
                &RenderOptions {
                    current_module: Some(current),
                    module_url_prefix: "",
                },
                &resolver,
                &source_id_resolver,
            )
        } else {
            virtual_module_fragment(&module.logical_path, &modules)
        };
        pages.push(RenderedPageRecord {
            module_segments: module.logical_path.segments().to_vec(),
            fragment,
        });
    }
    Ok(RenderedWorkspaceRecord {
        site_name,
        pages,
        evaluation_diagnostics,
    })
}

fn virtual_module_fragment(current: &ModulePath, modules: &[&notist_analysis::Module]) -> String {
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
            escape_html(
                &mut output,
                path.segments().last().expect("child has a name"),
            );
            output.push_str("</a></li>");
        }
    }
    output.push_str("</ul>");
    output
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
        DiagnosticKind::UnknownFunction => "unknown-function",
        DiagnosticKind::DuplicateFunction => "duplicate-function",
        DiagnosticKind::UnresolvedName => "unresolved-name",
        DiagnosticKind::InvalidFunction => "invalid-function",
        DiagnosticKind::InvalidArguments => "invalid-arguments",
        DiagnosticKind::TypeMismatch => "type-mismatch",
        DiagnosticKind::Evaluation => "evaluation",
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
        let root = tempfile::TempDir::new().unwrap();
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
    fn edit_plans_enforce_fingerprints_and_idempotency() {
        let root = tempfile::TempDir::new().unwrap();
        let path = root.path().join("README.not");
        fs::write(&path, "one").unwrap();
        let path = dunce::canonicalize(path).unwrap();
        let service = NotistService::new();
        let opened = service
            .execute(CoreRequest::OpenView {
                root: root.path().to_path_buf(),
                kind: ProtocolViewKind::Disk,
            })
            .unwrap();
        let revision = opened.snapshot.revision;
        let CoreResponse::Opened { view_id, .. } = opened.response else {
            panic!("expected opened view")
        };
        let plan = service
            .execute(CoreRequest::ProposeEdit {
                view_id,
                base_revision: revision,
                operations: vec![EditOperation {
                    path: path.clone(),
                    range: ByteRange { start: 0, end: 3 },
                    replacement: "two".into(),
                }],
            })
            .unwrap();
        let CoreResponse::EditPlan(plan) = plan.response else {
            panic!("expected edit plan")
        };
        assert!(plan.diagnostics.is_empty());
        let request = CoreRequest::ApplyEdit {
            view_id,
            plan_hash: plan.plan_hash.clone(),
            expected_fingerprints: plan.affected_sources.clone(),
            idempotency_key: "test-edit".into(),
        };
        let first = service.execute(request.clone()).unwrap();
        let second = service.execute(request).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two");
        let CoreResponse::EditApplied(first) = first.response else {
            panic!("expected applied edit")
        };
        let CoreResponse::EditApplied(second) = second.response else {
            panic!("expected idempotent applied edit")
        };
        assert_eq!(first, second);
    }

    #[test]
    fn edit_apply_rejects_sources_changed_after_proposal() {
        let root = tempfile::TempDir::new().unwrap();
        let path = root.path().join("README.not");
        fs::write(&path, "one").unwrap();
        let path = dunce::canonicalize(path).unwrap();
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
        let plan = service
            .execute(CoreRequest::ProposeEdit {
                view_id,
                base_revision: opened.snapshot.revision,
                operations: vec![EditOperation {
                    path: path.clone(),
                    range: ByteRange { start: 0, end: 3 },
                    replacement: "two".into(),
                }],
            })
            .unwrap();
        let CoreResponse::EditPlan(plan) = plan.response else {
            panic!("expected edit plan")
        };
        fs::write(&path, "changed").unwrap();
        let error = service
            .execute(CoreRequest::ApplyEdit {
                view_id,
                plan_hash: plan.plan_hash,
                expected_fingerprints: plan.affected_sources,
                idempotency_key: "stale-edit".into(),
            })
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read_to_string(path).unwrap(), "changed");
    }

    #[test]
    fn daemon_serializes_preconditioned_source_renames() {
        let root = tempfile::TempDir::new().unwrap();
        let from = root.path().join("old.not");
        let to = root.path().join("new.not");
        fs::write(&from, "content").unwrap();
        let from = dunce::canonicalize(from).unwrap();
        let service = NotistService::new();
        let opened = service
            .execute(CoreRequest::OpenView {
                root: root.path().to_path_buf(),
                kind: ProtocolViewKind::Disk,
            })
            .unwrap();
        let CoreResponse::Opened { view_id, .. } = opened.response else {
            panic!("expected open view")
        };
        let fingerprint = service
            .execute(CoreRequest::FingerprintSource {
                view_id,
                path: from.clone(),
            })
            .unwrap();
        let CoreResponse::SourceFingerprint(Some(fingerprint)) = fingerprint.response else {
            panic!("expected source fingerprint")
        };
        let request = CoreRequest::RenameSource {
            view_id,
            from: from.clone(),
            to: to.clone(),
            expected_fingerprint: fingerprint.fingerprint,
            idempotency_key: "rename-test".into(),
        };
        let first = service.execute(request.clone()).unwrap();
        let second = service.execute(request).unwrap();

        assert!(!from.exists());
        assert_eq!(fs::read_to_string(&to).unwrap(), "content");
        let CoreResponse::SourceRenamed(first) = first.response else {
            panic!("expected source rename")
        };
        let CoreResponse::SourceRenamed(second) = second.response else {
            panic!("expected idempotent source rename")
        };
        assert_eq!(first, second);
    }

    #[test]
    fn protocol_configuration_projects_plugin_schema_without_running_plugin_code() {
        let root = tempfile::TempDir::new().unwrap();
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
}
