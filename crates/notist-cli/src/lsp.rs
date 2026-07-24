use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest, References, Request as _,
    Shutdown, WorkspaceSymbolRequest,
};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams, CompletionResponse,
    CompletionTextEdit, Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverContents, HoverParams, HoverProviderCapability,
    InitializeParams, Location, MarkupContent, MarkupKind, NumberOrString, OneOf, Position,
    PositionEncodingKind, PublishDiagnosticsParams, Range, ReferenceParams, ServerCapabilities,
    SymbolKind, TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, Uri,
    WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use notify_debouncer_mini::notify::RecursiveMode;
use notify_debouncer_mini::{DebounceEventResult, new_debouncer};
use notist_analysis::{
    AnalyzerView, CompletionKind, DiagnosticKind, DocumentVersions, SourceInput, SourceOverlays,
    VaultEngine, WorkspaceSnapshot, discover_vault_roots,
};
use notist_model::TextRange;

#[cfg(test)]
use notist_analysis::LineIndex;
#[cfg(test)]
use notist_model::ModulePath;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

const URI_PATH_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b']');

const WORKSPACE_CHANGED_NOTIFICATION: &str = "notist/workspaceChanged";

pub fn run() -> Result<ExitCode, Box<dyn Error>> {
    let (connection, io_threads) = Connection::stdio();
    let capabilities = serde_json::to_value(server_capabilities())?;
    let initialization = connection.initialize(capabilities)?;
    let initialization: InitializeParams = serde_json::from_value(initialization)?;
    let root = workspace_root(&initialization)?;
    let mut state = ServerState::new(root)?;
    let watcher_sender = connection.sender.clone();
    let mut watcher = new_debouncer(
        Duration::from_millis(250),
        move |result: DebounceEventResult| {
            if let Ok(events) = result
                && !events.is_empty()
            {
                let _ = watcher_sender.send(Message::Notification(Notification::new(
                    WORKSPACE_CHANGED_NOTIFICATION.into(),
                    serde_json::Value::Null,
                )));
            }
        },
    )?;
    watcher
        .watcher()
        .watch(&state.root, RecursiveMode::Recursive)?;

    main_loop(&connection, &mut state)?;
    drop(watcher);
    io_threads.join()?;
    Ok(ExitCode::SUCCESS)
}

fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        position_encoding: Some(PositionEncodingKind::UTF16),
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        completion_provider: Some(CompletionOptions {
            resolve_provider: Some(false),
            trigger_characters: Some(vec![
                "[".into(),
                ":".into(),
                "#".into(),
                "(".into(),
                ",".into(),
            ]),
            ..CompletionOptions::default()
        }),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        ..ServerCapabilities::default()
    }
}

fn workspace_root(params: &InitializeParams) -> Result<PathBuf, Box<dyn Error>> {
    #[allow(deprecated)]
    let uri = params
        .workspace_folders
        .as_ref()
        .and_then(|folders| folders.first())
        .map(|folder| &folder.uri)
        .or(params.root_uri.as_ref());
    let root = match uri {
        Some(uri) => uri_to_file_path(uri)?,
        None => std::env::current_dir()?,
    };
    Ok(dunce::canonicalize(root)?)
}

fn main_loop(connection: &Connection, state: &mut ServerState) -> Result<(), Box<dyn Error>> {
    for message in &connection.receiver {
        match message {
            Message::Request(request) => {
                if connection.handle_shutdown(&request)? {
                    return Ok(());
                }
                handle_request(connection, state, request)?;
            }
            Message::Notification(notification) => {
                handle_notification(state, notification)?;
                publish_diagnostics(connection, state)?;
            }
            Message::Response(_) => {}
        }
    }
    Ok(())
}

fn handle_request(
    connection: &Connection,
    state: &ServerState,
    request: Request,
) -> Result<(), Box<dyn Error>> {
    let id = request.id.clone();
    let result = match request.method.as_str() {
        GotoDefinition::METHOD => parse_params(request.params)
            .and_then(|params: GotoDefinitionParams| definition(state, params))
            .and_then(to_json),
        References::METHOD => parse_params(request.params)
            .and_then(|params: ReferenceParams| references(state, params))
            .and_then(to_json),
        Completion::METHOD => parse_params(request.params)
            .and_then(|params: CompletionParams| completion(state, params))
            .and_then(to_json),
        HoverRequest::METHOD => parse_params(request.params)
            .and_then(|params: HoverParams| hover(state, params))
            .and_then(to_json),
        DocumentSymbolRequest::METHOD => parse_params(request.params)
            .and_then(|params: DocumentSymbolParams| document_symbols(state, params))
            .and_then(to_json),
        WorkspaceSymbolRequest::METHOD => parse_params(request.params)
            .and_then(|params: WorkspaceSymbolParams| workspace_symbols(state, params))
            .and_then(to_json),
        Shutdown::METHOD => Ok(serde_json::Value::Null),
        _ => {
            connection.sender.send(Message::Response(Response::new_err(
                id,
                ErrorCode::MethodNotFound as i32,
                format!("unsupported request `{}`", request.method),
            )))?;
            return Ok(());
        }
    };

    let response = match result {
        Ok(value) => Response::new_ok(id, value),
        Err(message) => Response::new_err(id, ErrorCode::InvalidParams as i32, message),
    };
    connection.sender.send(Message::Response(response))?;
    Ok(())
}

fn parse_params<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|error| error.to_string())
}

fn to_json<T: serde::Serialize>(value: T) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|error| error.to_string())
}

fn handle_notification(
    state: &mut ServerState,
    notification: Notification,
) -> Result<(), Box<dyn Error>> {
    let changed = match notification.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let params: DidOpenTextDocumentParams = serde_json::from_value(notification.params)?;
            state.open(params)?;
            true
        }
        DidChangeTextDocument::METHOD => {
            let params: DidChangeTextDocumentParams = serde_json::from_value(notification.params)?;
            state.change(params)?
        }
        DidSaveTextDocument::METHOD => {
            let params: DidSaveTextDocumentParams = serde_json::from_value(notification.params)?;
            state.save(params)?
        }
        DidCloseTextDocument::METHOD => {
            let params: DidCloseTextDocumentParams = serde_json::from_value(notification.params)?;
            state.close(params)?;
            true
        }
        WORKSPACE_CHANGED_NOTIFICATION => true,
        _ => false,
    };

    if changed && let Err(error) = state.rebuild() {
        eprintln!("notist lsp: workspace rebuild failed: {error}");
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct OpenDocument {
    version: i32,
    source: Arc<str>,
}

struct ServerState {
    root: PathBuf,
    documents: BTreeMap<PathBuf, OpenDocument>,
    vaults: BTreeMap<PathBuf, AnalyzerView>,
    published_paths: BTreeSet<PathBuf>,
}

impl ServerState {
    fn new(root: PathBuf) -> Result<Self, Box<dyn Error>> {
        let mut state = Self {
            root,
            documents: BTreeMap::new(),
            vaults: BTreeMap::new(),
            published_paths: BTreeSet::new(),
        };
        state.rebuild()?;
        Ok(state)
    }

    fn open(&mut self, params: DidOpenTextDocumentParams) -> Result<(), Box<dyn Error>> {
        let path = normalize_uri_path(&self.root, &params.text_document.uri)?;
        self.documents.insert(
            path,
            OpenDocument {
                version: params.text_document.version,
                source: Arc::from(params.text_document.text),
            },
        );
        Ok(())
    }

    fn change(&mut self, params: DidChangeTextDocumentParams) -> Result<bool, Box<dyn Error>> {
        let path = normalize_uri_path(&self.root, &params.text_document.uri)?;
        let Some(change) = params.content_changes.last() else {
            return Ok(false);
        };
        if change.range.is_some() {
            return Err("client sent an incremental edit to a full-sync server".into());
        }
        self.documents.insert(
            path,
            OpenDocument {
                version: params.text_document.version,
                source: Arc::from(change.text.clone()),
            },
        );
        Ok(true)
    }

    fn save(&mut self, params: DidSaveTextDocumentParams) -> Result<bool, Box<dyn Error>> {
        let path = normalize_uri_path(&self.root, &params.text_document.uri)?;
        let Some(source) = params.text else {
            return Ok(false);
        };
        let version = self
            .documents
            .get(&path)
            .map_or(0, |document| document.version);
        self.documents.insert(
            path,
            OpenDocument {
                version,
                source: Arc::from(source),
            },
        );
        Ok(true)
    }

    fn close(&mut self, params: DidCloseTextDocumentParams) -> Result<(), Box<dyn Error>> {
        let path = normalize_uri_path(&self.root, &params.text_document.uri)?;
        self.documents.remove(&path);
        Ok(())
    }

    fn rebuild(&mut self) -> Result<(), Box<dyn Error>> {
        let mut roots = discover_vault_roots(&self.root)?;
        if roots.is_empty() {
            roots.push(self.root.clone());
        }

        let mut previous = std::mem::take(&mut self.vaults);
        let mut vaults = BTreeMap::new();
        for root in &roots {
            let overlays: SourceOverlays = self
                .documents
                .iter()
                .filter(|(path, _)| assigned_vault_root(path, &roots) == Some(root))
                .map(|(path, document)| (path.clone(), document.source.clone()))
                .collect();
            let document_versions: DocumentVersions = self
                .documents
                .iter()
                .filter(|(path, _)| assigned_vault_root(path, &roots) == Some(root))
                .map(|(path, document)| (path.clone(), i64::from(document.version)))
                .collect();
            let view = if let Some(mut view) = previous.remove(root) {
                view.replace_inputs(overlays, document_versions)?;
                view
            } else {
                VaultEngine::open(root)?.view_with_versions(overlays, document_versions)?
            };
            vaults.insert(root.clone(), view);
        }
        self.vaults = vaults;
        Ok(())
    }

    fn document_version(&self, path: &Path) -> Option<i32> {
        self.documents.get(path).map(|document| document.version)
    }

    fn workspace_for_source(&self, path: &Path) -> Option<&WorkspaceSnapshot> {
        let root = assigned_vault_root(path, self.vaults.keys())?;
        self.vaults.get(root).map(AnalyzerView::current)
    }
}

#[allow(deprecated)]
fn document_symbols(
    state: &ServerState,
    params: DocumentSymbolParams,
) -> Result<Option<DocumentSymbolResponse>, String> {
    let path = normalize_uri_path(&state.root, &params.text_document.uri)
        .map_err(|error| error.to_string())?;
    let Some(workspace) = state.workspace_for_source(&path) else {
        return Ok(None);
    };
    let Some(file_id) = workspace.file_id(&path) else {
        return Ok(None);
    };
    let Some(source) = workspace.source(file_id) else {
        return Ok(Some(DocumentSymbolResponse::Nested(Vec::new())));
    };
    let symbols = workspace
        .document_symbols(file_id)
        .into_iter()
        .map(|symbol| {
            let range = lsp_range(source, symbol.range);
            let name = symbol.name;
            let name = if name.trim().is_empty() {
                "Untitled heading".into()
            } else {
                name.trim().to_owned()
            };
            (
                symbol.level,
                DocumentSymbol {
                    name,
                    detail: Some(format!("Heading {}", symbol.level)),
                    kind: SymbolKind::NAMESPACE,
                    tags: None,
                    deprecated: None,
                    range,
                    selection_range: range,
                    children: None,
                },
            )
        })
        .collect();
    Ok(Some(DocumentSymbolResponse::Nested(nest_heading_symbols(
        symbols,
    ))))
}

fn nest_heading_symbols(symbols: Vec<(u8, DocumentSymbol)>) -> Vec<DocumentSymbol> {
    fn finish_one(stack: &mut Vec<(u8, DocumentSymbol)>, roots: &mut Vec<DocumentSymbol>) {
        let (_, symbol) = stack.pop().unwrap();
        if let Some((_, parent)) = stack.last_mut() {
            parent.children.get_or_insert_with(Vec::new).push(symbol);
        } else {
            roots.push(symbol);
        }
    }

    let mut roots = Vec::new();
    let mut stack: Vec<(u8, DocumentSymbol)> = Vec::new();
    for (level, symbol) in symbols {
        while stack
            .last()
            .is_some_and(|(parent_level, _)| *parent_level >= level)
        {
            finish_one(&mut stack, &mut roots);
        }
        stack.push((level, symbol));
    }
    while !stack.is_empty() {
        finish_one(&mut stack, &mut roots);
    }
    roots
}

#[allow(deprecated)]
fn workspace_symbols(
    state: &ServerState,
    params: WorkspaceSymbolParams,
) -> Result<Option<WorkspaceSymbolResponse>, String> {
    let mut symbols = Vec::new();
    for view in state.vaults.values() {
        let snapshot = view.current();
        for symbol in snapshot.workspace_symbols(&params.query) {
            let Some(source) = snapshot.source(symbol.file_id) else {
                continue;
            };
            let uri = file_path_to_uri(&source.canonical_path)?;
            symbols.push(lsp_types::SymbolInformation {
                name: symbol.name,
                kind: SymbolKind::FILE,
                tags: None,
                deprecated: None,
                location: Location::new(uri, Range::new(Position::new(0, 0), Position::new(0, 0))),
                container_name: Some("Notist vault".into()),
            });
        }
    }
    symbols.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(Some(WorkspaceSymbolResponse::Flat(symbols)))
}

fn assigned_vault_root<'a>(
    path: &Path,
    roots: impl IntoIterator<Item = &'a PathBuf>,
) -> Option<&'a PathBuf> {
    roots
        .into_iter()
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.components().count())
}

fn normalize_uri_path(root: &Path, uri: &Uri) -> Result<PathBuf, Box<dyn Error>> {
    let path = uri_to_file_path(uri)?;
    if path.exists() {
        return Ok(dunce::canonicalize(path)?);
    }
    let parent = path
        .parent()
        .ok_or_else(|| format!("document path `{}` has no parent", path.display()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("document path `{}` has no file name", path.display()))?;
    let parent = if parent.exists() {
        dunce::canonicalize(parent)?
    } else {
        root.to_path_buf()
    };
    Ok(parent.join(file_name))
}

fn uri_to_file_path(uri: &Uri) -> Result<PathBuf, Box<dyn Error>> {
    if uri.scheme().map(|scheme| scheme.as_str()) != Some("file") {
        return Err(format!("URI {:?} is not a file URI", uri.as_str()).into());
    }
    if uri
        .authority()
        .is_some_and(|authority| !authority.as_str().is_empty())
    {
        return Err(format!("file URI {:?} has an unsupported authority", uri.as_str()).into());
    }
    let path = uri.path().as_estr().decode().into_string()?;

    #[cfg(target_os = "windows")]
    let path = {
        let path = path
            .strip_prefix('/')
            .filter(|path| path.as_bytes().get(1) == Some(&b':'))
            .unwrap_or(&path);
        PathBuf::from(path.replace('/', "\\"))
    };
    #[cfg(not(target_os = "windows"))]
    let path = PathBuf::from(path.as_ref());

    Ok(path)
}

fn file_path_to_uri(path: &Path) -> Result<Uri, String> {
    let mut path = path.to_string_lossy().replace('\\', "/");
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    let path = utf8_percent_encode(&path, URI_PATH_ENCODE_SET);
    Uri::from_str(&format!("file://{path}"))
        .map_err(|error| format!("cannot convert {} to a file URI: {error}", path))
}

fn publish_diagnostics(
    connection: &Connection,
    state: &mut ServerState,
) -> Result<(), Box<dyn Error>> {
    let current_paths: BTreeSet<_> = state
        .vaults
        .values()
        .flat_map(|view| view.current().modules())
        .filter_map(|module| module.source_path.clone())
        .collect();
    let paths: BTreeSet<_> = current_paths
        .union(&state.published_paths)
        .cloned()
        .collect();
    for path in paths {
        let diagnostics = diagnostics_for_path(state, &path);
        let uri = file_path_to_uri(&path)?;
        let params = PublishDiagnosticsParams::new(uri, diagnostics, state.document_version(&path));
        connection
            .sender
            .send(Message::Notification(Notification::new(
                PublishDiagnostics::METHOD.to_owned(),
                params,
            )))?;
    }
    state.published_paths = current_paths;
    Ok(())
}

fn diagnostics_for_path(state: &ServerState, path: &Path) -> Vec<Diagnostic> {
    let Some(workspace) = state.workspace_for_source(path) else {
        return Vec::new();
    };
    let Some(module) = workspace.module_for_source(path) else {
        return Vec::new();
    };
    let Some(file_id) = module.file_id else {
        return Vec::new();
    };
    let Some(source) = workspace.source(file_id) else {
        return Vec::new();
    };

    workspace
        .diagnostics_for(file_id)
        .map(|diagnostic| {
            lsp_diagnostic(
                source,
                diagnostic.range.unwrap_or(TextRange::new(0, 0)),
                diagnostic_code(&diagnostic.kind),
                diagnostic.message.clone(),
            )
        })
        .collect()
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
    }
}

fn lsp_diagnostic(
    source: &SourceInput,
    range: TextRange,
    code: &str,
    message: String,
) -> Diagnostic {
    Diagnostic {
        range: lsp_range(source, range),
        severity: Some(DiagnosticSeverity::ERROR),
        code: Some(NumberOrString::String(code.into())),
        source: Some("notist".into()),
        message,
        ..Diagnostic::default()
    }
}

fn definition(
    state: &ServerState,
    params: GotoDefinitionParams,
) -> Result<Option<GotoDefinitionResponse>, String> {
    let position = params.text_document_position_params;
    let Some((_path, workspace, file_id, _module, offset)) = source_position(state, &position)
    else {
        return Ok(None);
    };
    let Some(definition) = workspace.definition_at(file_id, offset) else {
        return Ok(None);
    };
    let Some(target_file_id) = definition.file_id else {
        return Ok(None);
    };
    let Some(target_source) = workspace.source(target_file_id) else {
        return Ok(None);
    };
    let uri = file_path_to_uri(&target_source.canonical_path)?;
    Ok(Some(GotoDefinitionResponse::Scalar(Location::new(
        uri,
        lsp_range(
            target_source,
            definition.range.unwrap_or(TextRange::new(0, 0)),
        ),
    ))))
}

fn references(
    state: &ServerState,
    params: ReferenceParams,
) -> Result<Option<Vec<Location>>, String> {
    let position = params.text_document_position;
    let Some((_path, workspace, file_id, _module, offset)) = source_position(state, &position)
    else {
        return Ok(None);
    };
    let Some(target) = workspace.reference_target_at(file_id, offset) else {
        return Ok(None);
    };
    let mut locations = Vec::new();
    if params.context.include_declaration {
        let declaration = target
            .annotation
            .as_ref()
            .and_then(|annotation| {
                workspace.label(
                    &workspace.module_by_id(annotation.module_id)?.logical_path,
                    &annotation.name,
                )
            })
            .map(|label| (label.file_id, label.range))
            .or_else(|| {
                let module = workspace.module_by_id(target.module_id)?;
                Some((module.file_id?, TextRange::new(0, 0)))
            });
        if let Some((declaration_file, range)) = declaration
            && let Some(source) = workspace.source(declaration_file)
        {
            let uri = file_path_to_uri(&source.canonical_path)?;
            locations.push(Location::new(uri, lsp_range(source, range)));
        }
    }
    for result in workspace.references_for_target(&target) {
        let reference = result.value;
        let Some(source) = workspace.source(reference.source_file_id) else {
            continue;
        };
        let uri = file_path_to_uri(&reference.source_path)?;
        locations.push(Location::new(uri, lsp_range(source, reference.range)));
    }
    Ok(Some(locations))
}

fn completion(
    state: &ServerState,
    params: CompletionParams,
) -> Result<Option<CompletionResponse>, String> {
    let position = params.text_document_position;
    let Some((_path, workspace, file_id, _module, offset)) = source_position(state, &position)
    else {
        return Ok(None);
    };
    let Some(source_input) = workspace.source(file_id) else {
        return Ok(None);
    };
    let items = workspace
        .completions_at(file_id, offset)
        .into_iter()
        .map(|candidate| CompletionItem {
            label: candidate.label,
            kind: Some(match candidate.kind {
                CompletionKind::Module => CompletionItemKind::MODULE,
                CompletionKind::Function => CompletionItemKind::FUNCTION,
                CompletionKind::Parameter => CompletionItemKind::FIELD,
            }),
            detail: Some(candidate.detail),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range: lsp_range(source_input, candidate.replacement),
                new_text: candidate.insert_text,
            })),
            ..CompletionItem::default()
        })
        .collect::<Vec<_>>();
    Ok((!items.is_empty()).then_some(CompletionResponse::Array(items)))
}

fn hover(state: &ServerState, params: HoverParams) -> Result<Option<Hover>, String> {
    let position = params.text_document_position_params;
    let Some((_path, workspace, file_id, _module, offset)) = source_position(state, &position)
    else {
        return Ok(None);
    };
    let Some(source_input) = workspace.source(file_id) else {
        return Ok(None);
    };
    let Some(hover) = workspace.hover_at(file_id, offset) else {
        return Ok(None);
    };
    Ok(Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: hover.contents,
        }),
        range: Some(lsp_range(source_input, hover.range)),
    }))
}

fn source_position<'a>(
    state: &'a ServerState,
    params: &lsp_types::TextDocumentPositionParams,
) -> Option<(
    PathBuf,
    &'a WorkspaceSnapshot,
    notist_analysis::FileId,
    &'a notist_analysis::Module,
    usize,
)> {
    let path = normalize_uri_path(&state.root, &params.text_document.uri).ok()?;
    let workspace = state.workspace_for_source(&path)?;
    let module = workspace.module_for_source(&path)?;
    let file_id = module.file_id?;
    let source = workspace.source(file_id)?;
    let offset = source.line_index.offset_utf16(
        &source.text,
        params.position.line,
        params.position.character,
    )?;
    Some((path, workspace, file_id, module, offset))
}

fn lsp_position(source: &SourceInput, offset: usize) -> Position {
    let (line, character) = source
        .line_index
        .utf16_position(&source.text, offset)
        .unwrap_or((0, 0));
    Position::new(line, character)
}

fn lsp_range(source: &SourceInput, range: TextRange) -> Range {
    Range::new(
        lsp_position(source, range.start),
        lsp_position(source, range.end),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_server::RequestId;
    use lsp_types::{
        CompletionContext, CompletionTriggerKind, GotoDefinitionParams, PartialResultParams,
        TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams,
        WorkDoneProgressParams,
    };
    use std::fs;
    use std::time::Duration;

    #[test]
    fn converts_utf8_offsets_and_utf16_positions() {
        let source = "a😀中\r\nnext";
        let index = LineIndex::new(source);

        assert_eq!(index.utf16_position(source, "a😀".len()), Some((0, 3)));
        assert_eq!(
            index.utf16_position(source, "a😀中\r\n".len()),
            Some((1, 0))
        );
        assert_eq!(index.offset_utf16(source, 0, 3), Some("a😀".len()));
        assert_eq!(index.offset_utf16(source, 1, 2), Some("a😀中\r\nne".len()));
    }

    #[test]
    fn converts_file_paths_and_new_uri_type() {
        let path = dunce::canonicalize(std::env::current_dir().unwrap())
            .unwrap()
            .join("space 文档.not");
        let uri = file_path_to_uri(&path).unwrap();

        assert!(uri.as_str().starts_with("file:///"));
        assert!(uri.as_str().contains("space%20"));
        assert_eq!(uri_to_file_path(&uri).unwrap(), path);
    }

    #[test]
    fn exposes_document_and_workspace_symbols() {
        let root = tempfile::TempDir::new().unwrap();
        fs::write(
            root.path().join("README.not"),
            "= Surface title\n== Nested title\n\n```not\n= Hidden example\n```\n\n#heading[Explicit title]\n#code(text=\"fn main() {}\", lang=\"rust\", block=true)",
        )
        .unwrap();
        fs::write(root.path().join("child.not"), "child").unwrap();
        let root_path = dunce::canonicalize(root.path()).unwrap();
        let readme_path = dunce::canonicalize(root.path().join("README.not")).unwrap();
        let state = ServerState::new(root_path).unwrap();
        let uri = file_path_to_uri(&readme_path).unwrap();

        let document = document_symbols(
            &state,
            DocumentSymbolParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            },
        )
        .unwrap()
        .unwrap();
        let DocumentSymbolResponse::Nested(document) = document else {
            panic!("expected nested document symbols");
        };
        assert_eq!(
            document
                .iter()
                .map(|symbol| symbol.name.as_str())
                .collect::<Vec<_>>(),
            ["Surface title", "Explicit title"]
        );
        assert_eq!(
            document[0].children.as_ref().unwrap()[0].name,
            "Nested title"
        );

        let workspace = workspace_symbols(
            &state,
            WorkspaceSymbolParams {
                query: "child".into(),
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            },
        )
        .unwrap()
        .unwrap();
        let WorkspaceSymbolResponse::Flat(workspace) = workspace else {
            panic!("expected flat workspace symbols");
        };
        assert_eq!(workspace.len(), 1);
        assert_eq!(workspace[0].name, "vault::child");
    }

    #[test]
    fn argument_completion_inside_unknown_nested_call_offers_no_outer_parameters() {
        let root = tempfile::TempDir::new().unwrap();
        fs::write(root.path().join("README.not"), "#heading(level=missing())").unwrap();
        let root_path = dunce::canonicalize(root.path()).unwrap();
        let state = ServerState::new(root_path).unwrap();
        let path = dunce::canonicalize(root.path().join("README.not")).unwrap();
        let uri = file_path_to_uri(&path).unwrap();
        let params = CompletionParams {
            text_document_position: lsp_types::TextDocumentPositionParams::new(
                lsp_types::TextDocumentIdentifier::new(uri),
                Position::new(0, 23),
            ),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: None,
        };

        // `missing` is not a known function, so there are no argument candidates;
        // crucially the outer `heading`'s `level` must not be offered.
        assert!(completion(&state, params).unwrap().is_none());
    }

    #[test]
    fn completion_uses_unsaved_workspace_sources() {
        let root = tempfile::TempDir::new().unwrap();
        fs::write(root.path().join("README.not"), "[[ch]]").unwrap();
        fs::write(root.path().join("child.not"), "child").unwrap();
        let root_path = dunce::canonicalize(root.path()).unwrap();
        let mut state = ServerState::new(root_path.clone()).unwrap();
        let path = dunce::canonicalize(root.path().join("README.not")).unwrap();
        state.documents.insert(
            path.clone(),
            OpenDocument {
                version: 1,
                source: Arc::from("[[ch]]"),
            },
        );
        state.rebuild().unwrap();
        let uri = file_path_to_uri(&path).unwrap();
        let params = CompletionParams {
            text_document_position: lsp_types::TextDocumentPositionParams::new(
                lsp_types::TextDocumentIdentifier::new(uri),
                Position::new(0, 4),
            ),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: None,
        };

        let Some(CompletionResponse::Array(items)) = completion(&state, params).unwrap() else {
            panic!("expected completion items");
        };
        assert!(items.iter().any(|item| item.label == "child"));
    }

    #[test]
    fn keeps_marked_vaults_independent_within_one_worktree() {
        let root = tempfile::TempDir::new().unwrap();
        for vault in ["docs", "notes"] {
            fs::create_dir(root.path().join(vault)).unwrap();
            fs::write(
                root.path().join(vault).join(notist_analysis::MANIFEST_FILE),
                "",
            )
            .unwrap();
            fs::write(root.path().join(vault).join("README.not"), "").unwrap();
        }
        fs::write(root.path().join("docs/guide.not"), "guide").unwrap();
        fs::write(root.path().join("notes/private.not"), "private").unwrap();
        let root_path = dunce::canonicalize(root.path()).unwrap();
        let mut state = ServerState::new(root_path).unwrap();
        let docs_readme = dunce::canonicalize(root.path().join("docs/README.not")).unwrap();
        let notes_readme = dunce::canonicalize(root.path().join("notes/README.not")).unwrap();

        assert_eq!(state.vaults.len(), 2);
        let docs = state.workspace_for_source(&docs_readme).unwrap();
        assert!(
            docs.module(&ModulePath::from_segments(["guide".into()]))
                .is_some()
        );
        assert!(
            docs.module(&ModulePath::from_segments(["private".into()]))
                .is_none()
        );
        let notes = state.workspace_for_source(&notes_readme).unwrap();
        assert!(
            notes
                .module(&ModulePath::from_segments(["private".into()]))
                .is_some()
        );

        let draft = root.path().join("docs/draft.not");
        state.documents.insert(
            draft.clone(),
            OpenDocument {
                version: 1,
                source: Arc::from("draft"),
            },
        );
        state.rebuild().unwrap();
        let docs = state.workspace_for_source(&draft).unwrap();
        assert!(
            docs.module(&ModulePath::from_segments(["draft".into()]))
                .is_some()
        );
        let notes = state.workspace_for_source(&notes_readme).unwrap();
        assert!(
            notes
                .module(&ModulePath::from_segments(["draft".into()]))
                .is_none()
        );
    }

    #[test]
    fn protocol_loop_serves_overlay_diagnostics_completion_hover_and_definition() {
        let root = tempfile::TempDir::new().unwrap();
        fs::write(root.path().join("README.not"), "disk").unwrap();
        fs::write(root.path().join("child.not"), "child").unwrap();
        let root_path = dunce::canonicalize(root.path()).unwrap();
        let readme_path = dunce::canonicalize(root.path().join("README.not")).unwrap();
        let child_path = dunce::canonicalize(root.path().join("child.not")).unwrap();
        let readme_uri = file_path_to_uri(&readme_path).unwrap();
        let source = "[[child]] #heading[] #missing[]";
        let state = ServerState::new(root_path).unwrap();
        let (server, client) = Connection::memory();
        let server_thread = std::thread::spawn(move || {
            let mut state = state;
            main_loop(&server, &mut state).unwrap();
        });

        client
            .sender
            .send(Message::Notification(Notification::new(
                DidOpenTextDocument::METHOD.into(),
                DidOpenTextDocumentParams {
                    text_document: TextDocumentItem::new(
                        readme_uri.clone(),
                        "notist".into(),
                        1,
                        source.into(),
                    ),
                },
            )))
            .unwrap();

        let mut saw_overlay_diagnostic = false;
        for _ in 0..2 {
            let Message::Notification(notification) = client
                .receiver
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
            else {
                panic!("expected diagnostics notification");
            };
            let params: PublishDiagnosticsParams =
                serde_json::from_value(notification.params).unwrap();
            if params.uri == readme_uri {
                saw_overlay_diagnostic = params
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.message == "unknown function `missing`");
            }
        }
        assert!(saw_overlay_diagnostic);

        let completion_id = RequestId::from(1);
        client
            .sender
            .send(Message::Request(Request::new(
                completion_id.clone(),
                Completion::METHOD.into(),
                CompletionParams {
                    text_document_position: TextDocumentPositionParams::new(
                        TextDocumentIdentifier::new(readme_uri.clone()),
                        Position::new(0, 15),
                    ),
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                    context: Some(CompletionContext {
                        trigger_kind: CompletionTriggerKind::INVOKED,
                        trigger_character: None,
                    }),
                },
            )))
            .unwrap();
        let response = recv_response(&client, &completion_id);
        let completion: Option<CompletionResponse> =
            serde_json::from_value(response_result(response)).unwrap();
        let Some(CompletionResponse::Array(items)) = completion else {
            panic!("expected completion array");
        };
        assert!(items.iter().any(|item| item.label == "heading"));

        let hover_id = RequestId::from(2);
        client
            .sender
            .send(Message::Request(Request::new(
                hover_id.clone(),
                HoverRequest::METHOD.into(),
                HoverParams {
                    text_document_position_params: TextDocumentPositionParams::new(
                        TextDocumentIdentifier::new(readme_uri.clone()),
                        Position::new(0, 12),
                    ),
                    work_done_progress_params: Default::default(),
                },
            )))
            .unwrap();
        let response = recv_response(&client, &hover_id);
        let hover: Option<Hover> = serde_json::from_value(response_result(response)).unwrap();
        assert!(matches!(
            hover.unwrap().contents,
            HoverContents::Markup(MarkupContent { value, .. }) if value.contains("#heading")
        ));

        let definition_id = RequestId::from(3);
        client
            .sender
            .send(Message::Request(Request::new(
                definition_id.clone(),
                GotoDefinition::METHOD.into(),
                GotoDefinitionParams {
                    text_document_position_params: TextDocumentPositionParams::new(
                        TextDocumentIdentifier::new(readme_uri),
                        Position::new(0, 3),
                    ),
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                },
            )))
            .unwrap();
        let response = recv_response(&client, &definition_id);
        let definition: Option<GotoDefinitionResponse> =
            serde_json::from_value(response_result(response)).unwrap();
        assert!(matches!(
            definition,
            Some(GotoDefinitionResponse::Scalar(Location { uri, .. }))
                if uri == file_path_to_uri(&child_path).unwrap()
        ));

        drop(client);
        server_thread.join().unwrap();
    }

    fn recv_response(connection: &Connection, id: &RequestId) -> Response {
        loop {
            let message = connection
                .receiver
                .recv_timeout(Duration::from_secs(2))
                .unwrap();
            if let Message::Response(response) = message
                && &response.id == id
            {
                return response;
            }
        }
    }

    fn response_result(response: Response) -> serde_json::Value {
        match response.response_result {
            Ok(result) => result,
            Err(error) => {
                panic!("unexpected LSP error response: {}", error.message)
            }
        }
    }
}
