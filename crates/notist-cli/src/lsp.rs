use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;
use std::sync::Arc;

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    Completion, GotoDefinition, HoverRequest, References, Request as _, Shutdown,
};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams, CompletionResponse,
    CompletionTextEdit, Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents, HoverParams,
    HoverProviderCapability, InitializeParams, Location, MarkupContent, MarkupKind, NumberOrString,
    OneOf, Position, PositionEncodingKind, PublishDiagnosticsParams, Range, ReferenceParams,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, Uri,
};
use notist_analysis::{DiagnosticKind, SourceOverlays, Workspace, discover_vault_roots};
use notist_eval::{DefaultValue, Evaluator, FunctionRegistry, FunctionSignature};
use notist_model::{ModulePath, TextRange};
use notist_syntax::Parse;
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

pub fn run() -> Result<ExitCode, Box<dyn Error>> {
    let (connection, io_threads) = Connection::stdio();
    let capabilities = serde_json::to_value(server_capabilities())?;
    let initialization = connection.initialize(capabilities)?;
    let initialization: InitializeParams = serde_json::from_value(initialization)?;
    let root = workspace_root(&initialization)?;
    let mut state = ServerState::new(root)?;

    main_loop(&connection, &mut state)?;
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
    vaults: BTreeMap<PathBuf, Workspace>,
    published_paths: BTreeSet<PathBuf>,
    functions: FunctionRegistry,
}

impl ServerState {
    fn new(root: PathBuf) -> Result<Self, Box<dyn Error>> {
        let mut state = Self {
            root,
            documents: BTreeMap::new(),
            vaults: BTreeMap::new(),
            published_paths: BTreeSet::new(),
            functions: FunctionRegistry::with_builtins(),
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

        let mut vaults = BTreeMap::new();
        for root in &roots {
            let overlays: SourceOverlays = self
                .documents
                .iter()
                .filter(|(path, _)| assigned_vault_root(path, &roots) == Some(root))
                .map(|(path, document)| (path.clone(), document.source.clone()))
                .collect();
            vaults.insert(root.clone(), Workspace::load_with_overlays(root, overlays)?);
        }
        self.vaults = vaults;
        Ok(())
    }

    fn document_version(&self, path: &Path) -> Option<i32> {
        self.documents.get(path).map(|document| document.version)
    }

    fn workspace_for_source(&self, path: &Path) -> Option<&Workspace> {
        let root = assigned_vault_root(path, self.vaults.keys())?;
        self.vaults.get(root)
    }
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
        .flat_map(Workspace::modules)
        .filter_map(|module| module.source_path.clone())
        .collect();
    let paths: BTreeSet<_> = current_paths
        .union(&state.published_paths)
        .cloned()
        .collect();
    let evaluator = Evaluator::default();

    for path in paths {
        let diagnostics = diagnostics_for_path(state, &evaluator, &path);
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

fn diagnostics_for_path(
    state: &ServerState,
    evaluator: &Evaluator,
    path: &Path,
) -> Vec<Diagnostic> {
    let Some(workspace) = state.workspace_for_source(path) else {
        return Vec::new();
    };
    let Some(module) = workspace.module_for_source(path) else {
        return Vec::new();
    };
    let Some(source) = module.source.as_deref() else {
        return Vec::new();
    };
    let line_index = LineIndex::new(source);
    let mut diagnostics = Vec::new();
    let mut seen = BTreeSet::new();

    for diagnostic in workspace
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.source_path.as_deref() == Some(path))
    {
        let range = diagnostic.range.unwrap_or(TextRange::new(0, 0));
        seen.insert((range.start, range.end, diagnostic.message.clone()));
        diagnostics.push(lsp_diagnostic(
            &line_index,
            range,
            diagnostic_code(&diagnostic.kind),
            diagnostic.message.clone(),
        ));
    }

    if let Some(parse) = &module.parse {
        for diagnostic in evaluator.evaluate_parsed(source, parse).diagnostics {
            if seen.insert((
                diagnostic.range.start,
                diagnostic.range.end,
                diagnostic.message.clone(),
            )) {
                diagnostics.push(lsp_diagnostic(
                    &line_index,
                    diagnostic.range,
                    "evaluation",
                    diagnostic.message,
                ));
            }
        }
    }
    diagnostics
}

fn diagnostic_code(kind: &DiagnosticKind) -> &'static str {
    match kind {
        DiagnosticKind::DuplicateModule => "duplicate-module",
        DiagnosticKind::InvalidSyntax => "invalid-syntax",
        DiagnosticKind::UnresolvedModule => "unresolved-module",
        DiagnosticKind::UnsupportedLabelReference => "unsupported-label-reference",
    }
}

fn lsp_diagnostic(
    line_index: &LineIndex<'_>,
    range: TextRange,
    code: &str,
    message: String,
) -> Diagnostic {
    Diagnostic {
        range: line_index.range(range),
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
    let Some((_path, workspace, module, offset)) = source_position(state, &position) else {
        return Ok(None);
    };
    let Some(link) = module
        .parse
        .as_ref()
        .and_then(|parse| link_at(parse, offset))
    else {
        return Ok(None);
    };
    let Some(target) = link.target.module.resolve_from(&module.logical_path) else {
        return Ok(None);
    };
    let Some(target_module) = workspace.module(&target) else {
        return Ok(None);
    };
    let Some(target_path) = &target_module.source_path else {
        return Ok(None);
    };
    let uri = file_path_to_uri(target_path)?;
    Ok(Some(GotoDefinitionResponse::Scalar(Location::new(
        uri,
        Range::new(Position::new(0, 0), Position::new(0, 0)),
    ))))
}

fn references(
    state: &ServerState,
    params: ReferenceParams,
) -> Result<Option<Vec<Location>>, String> {
    let position = params.text_document_position;
    let Some((_path, workspace, module, offset)) = source_position(state, &position) else {
        return Ok(None);
    };
    let Some(link) = module
        .parse
        .as_ref()
        .and_then(|parse| link_at(parse, offset))
    else {
        return Ok(None);
    };
    let Some(target) = link.target.module.resolve_from(&module.logical_path) else {
        return Ok(None);
    };
    let mut locations = Vec::new();
    if params.context.include_declaration
        && let Some(target_module) = workspace.module(&target)
        && let Some(path) = &target_module.source_path
    {
        let uri = file_path_to_uri(path)?;
        locations.push(Location::new(
            uri,
            Range::new(Position::new(0, 0), Position::new(0, 0)),
        ));
    }
    for reference in workspace
        .references()
        .iter()
        .filter(|reference| reference.target_module == target)
    {
        let Some(source_module) = workspace.module(&reference.source_module) else {
            continue;
        };
        let Some(source) = source_module.source.as_deref() else {
            continue;
        };
        let uri = file_path_to_uri(&reference.source_path)?;
        locations.push(Location::new(
            uri,
            LineIndex::new(source).range(reference.range),
        ));
    }
    Ok(Some(locations))
}

fn completion(
    state: &ServerState,
    params: CompletionParams,
) -> Result<Option<CompletionResponse>, String> {
    let position = params.text_document_position;
    let Some((_path, workspace, module, offset)) = source_position(state, &position) else {
        return Ok(None);
    };
    let Some(source) = module.source.as_deref() else {
        return Ok(None);
    };
    let parse = module.parse.as_ref().cloned().unwrap_or_default();
    let line_index = LineIndex::new(source);

    if is_in_raw_literal(&parse, offset) {
        return Ok(None);
    }
    if let Some((call, context)) = argument_completion_context(source, &parse, offset)
        && let Some(function) = state.functions.get(&call.name.value)
    {
        let mut items = Vec::new();
        let signature = function.signature();
        let used = used_argument_parameters(&signature, call);
        for parameter in completable_parameters(&signature, &used, &context.prefix) {
            items.push(CompletionItem {
                label: parameter.name.into(),
                kind: Some(CompletionItemKind::FIELD),
                detail: Some(parameter.ty.to_string()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: line_index.range(context.replace),
                    new_text: format!("{}=", parameter.name),
                })),
                ..CompletionItem::default()
            });
        }
        return Ok(Some(CompletionResponse::Array(items)));
    }
    if let Some(context) = wiki_completion_context(source, &parse, offset) {
        let mut items = Vec::new();
        for target in workspace.modules().map(|module| &module.logical_path) {
            if target == &module.logical_path {
                continue;
            }
            let reference =
                completion_module_reference(&module.logical_path, target, &context.prefix);
            if !starts_with_case_insensitive(&reference, &context.prefix) {
                continue;
            }
            items.push(CompletionItem {
                label: reference.clone(),
                kind: Some(CompletionItemKind::MODULE),
                detail: Some(target.to_string()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: line_index.range(context.replace),
                    new_text: reference,
                })),
                ..CompletionItem::default()
            });
        }
        items.sort_by(|left, right| left.label.cmp(&right.label));
        return Ok(Some(CompletionResponse::Array(items)));
    }

    if let Some(context) = function_completion_context(source, &parse, offset) {
        let mut functions: Vec<_> = state.functions.functions().collect();
        functions.sort_by_key(|function| function.name().to_owned());
        let items = functions
            .into_iter()
            .filter(|function| starts_with_case_insensitive(function.name(), &context.prefix))
            .map(|function| CompletionItem {
                label: function.name().to_owned(),
                kind: Some(CompletionItemKind::FUNCTION),
                detail: Some(format_signature(function.name(), &function.signature())),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: line_index.range(context.replace),
                    new_text: function.name().to_owned(),
                })),
                ..CompletionItem::default()
            })
            .collect();
        return Ok(Some(CompletionResponse::Array(items)));
    }
    Ok(None)
}

fn hover(state: &ServerState, params: HoverParams) -> Result<Option<Hover>, String> {
    let position = params.text_document_position_params;
    let Some((_path, workspace, module, offset)) = source_position(state, &position) else {
        return Ok(None);
    };
    let Some(source) = module.source.as_deref() else {
        return Ok(None);
    };
    let Some(parse) = &module.parse else {
        return Ok(None);
    };
    let line_index = LineIndex::new(source);

    if let Some(link) = link_at(parse, offset) {
        let Some(target) = link.target.module.resolve_from(&module.logical_path) else {
            return Ok(None);
        };
        let (kind, path) = match workspace.module(&target) {
            Some(target) => (
                if target.source_path.is_some() {
                    "source module"
                } else {
                    "virtual module"
                },
                target
                    .source_path
                    .as_ref()
                    .map(|path| format!("\n\n`{}`", path.display()))
                    .unwrap_or_default(),
            ),
            None => ("unresolved module", String::new()),
        };
        return Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!("**Module** `{target}`\n\n{kind}{path}"),
            }),
            range: Some(line_index.range(link.range)),
        }));
    }

    if let Some(call) = parse
        .calls
        .iter()
        .find(|call| contains(call.name.range, offset))
        && let Some(function) = state.functions.get(&call.name.value)
    {
        let signature = format_signature(function.name(), &function.signature());
        return Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!("```notist\n{signature}\n```"),
            }),
            range: Some(line_index.range(call.name.range)),
        }));
    }
    Ok(None)
}

fn source_position<'a>(
    state: &'a ServerState,
    params: &lsp_types::TextDocumentPositionParams,
) -> Option<(PathBuf, &'a Workspace, &'a notist_analysis::Module, usize)> {
    let path = normalize_uri_path(&state.root, &params.text_document.uri).ok()?;
    let workspace = state.workspace_for_source(&path)?;
    let module = workspace.module_for_source(&path)?;
    let source = module.source.as_deref()?;
    let offset = LineIndex::new(source).offset(params.position)?;
    Some((path, workspace, module, offset))
}

fn link_at(parse: &Parse, offset: usize) -> Option<&notist_syntax::WikiLink> {
    parse.links.iter().find(|link| contains(link.range, offset))
}

fn contains(range: TextRange, offset: usize) -> bool {
    range.start <= offset && offset < range.end
}

fn is_in_raw_literal(parse: &Parse, offset: usize) -> bool {
    parse.raw_literals.iter().any(|raw| {
        contains(raw.range, offset)
            || (raw.payload_range.end == raw.range.end && offset == raw.range.end)
    })
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
    if let Some(link) = parse.links.iter().find(|link| contains(link.range, offset)) {
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
        .calls
        .iter()
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

fn argument_completion_context<'a>(
    source: &str,
    parse: &'a Parse,
    offset: usize,
) -> Option<(&'a notist_syntax::Call, CompletionContext)> {
    let call = parse.calls.iter().find(|call| {
        call.arguments_range
            .is_some_and(|range| range.start <= offset && offset <= range.end)
    })?;
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

fn completable_parameters<'a>(
    signature: &'a FunctionSignature,
    used: &BTreeSet<&str>,
    prefix: &str,
) -> Vec<&'a notist_eval::Parameter> {
    signature
        .parameters
        .iter()
        .filter(|parameter| {
            !used.contains(parameter.name)
                && signature.trailing_content != Some(parameter.name)
                && starts_with_case_insensitive(parameter.name, prefix)
        })
        .collect()
}

fn used_argument_parameters<'a>(
    signature: &'a FunctionSignature,
    call: &'a notist_syntax::Call,
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
                used.insert(parameter.name);
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
    let trailing = signature.trailing_content.and_then(|name| {
        signature
            .parameters
            .iter()
            .find(|parameter| parameter.name == name)
    });
    let parameters = signature
        .parameters
        .iter()
        .filter(|parameter| signature.trailing_content != Some(parameter.name))
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

struct LineIndex<'a> {
    source: &'a str,
    line_starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    fn new(source: &'a str) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(
            source
                .bytes()
                .enumerate()
                .filter_map(|(offset, byte)| (byte == b'\n').then_some(offset + 1)),
        );
        Self {
            source,
            line_starts,
        }
    }

    fn position(&self, offset: usize) -> Position {
        let mut offset = offset.min(self.source.len());
        while !self.source.is_char_boundary(offset) {
            offset -= 1;
        }
        let line = self
            .line_starts
            .partition_point(|start| *start <= offset)
            .saturating_sub(1);
        let character = self.source[self.line_starts[line]..offset]
            .encode_utf16()
            .count();
        Position::new(line as u32, character as u32)
    }

    fn offset(&self, position: Position) -> Option<usize> {
        let start = *self.line_starts.get(position.line as usize)?;
        let end = self
            .line_starts
            .get(position.line as usize + 1)
            .copied()
            .unwrap_or(self.source.len());
        let line = &self.source[start..end];
        let mut utf16 = 0u32;
        for (byte, character) in line.char_indices() {
            if utf16 >= position.character {
                return Some(start + byte);
            }
            let next = utf16 + character.len_utf16() as u32;
            if position.character < next {
                return Some(start + byte);
            }
            utf16 = next;
        }
        (utf16 == position.character).then_some(end)
    }

    fn range(&self, range: TextRange) -> Range {
        Range::new(self.position(range.start), self.position(range.end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_server::RequestId;
    use lsp_types::{
        CompletionContext, CompletionTriggerKind, GotoDefinitionParams, TextDocumentIdentifier,
        TextDocumentItem, TextDocumentPositionParams,
    };
    use std::fs;
    use std::time::Duration;

    #[test]
    fn converts_utf8_offsets_and_utf16_positions() {
        let source = "a😀中\r\nnext";
        let index = LineIndex::new(source);

        assert_eq!(index.position("a😀".len()), Position::new(0, 3));
        assert_eq!(index.position("a😀中\r\n".len()), Position::new(1, 0));
        assert_eq!(index.offset(Position::new(0, 3)), Some("a😀".len()));
        assert_eq!(index.offset(Position::new(1, 2)), Some("a😀中\r\nne".len()));
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
    fn chooses_short_module_references() {
        let current = ModulePath::from_segments(["notes".into(), "today".into()]);
        let child = ModulePath::from_segments(["notes".into(), "today".into(), "details".into()]);
        let sibling = ModulePath::from_segments(["notes".into(), "index".into()]);

        assert_eq!(relative_module_reference(&current, &child), "details");
        assert_eq!(
            relative_module_reference(&current, &sibling),
            "super::index"
        );
        assert_eq!(
            completion_module_reference(&current, &child, "self::d"),
            "self::details"
        );
        assert_eq!(
            completion_module_reference(&current, &sibling, "vault::n"),
            "vault::notes::index"
        );
    }

    #[test]
    fn completes_empty_builtin_argument_lists() {
        let source = "#heading()[Title]";
        let parse = notist_syntax::parse(source);
        let (call, context) = argument_completion_context(source, &parse, 9).unwrap();

        assert_eq!(call.name.value, "heading");
        assert_eq!(context.prefix, "");
        assert_eq!(context.replace, TextRange::new(9, 9));
    }

    #[test]
    fn omits_trailing_content_from_argument_completion() {
        let registry = FunctionRegistry::with_builtins();
        let signature = registry.get("heading").unwrap().signature();
        let parameters = completable_parameters(&signature, &BTreeSet::new(), "");

        assert_eq!(
            parameters
                .into_iter()
                .map(|parameter| parameter.name)
                .collect::<Vec<_>>(),
            ["level"]
        );
    }

    #[test]
    fn omits_parameters_already_filled_positionally_from_completion() {
        let registry = FunctionRegistry::with_builtins();
        let signature = registry.get("raw").unwrap().signature();
        let parse = notist_syntax::parse("#raw(\"code\", )");
        let used = used_argument_parameters(&signature, &parse.calls[0]);
        let parameters = completable_parameters(&signature, &used, "");

        assert_eq!(
            parameters
                .into_iter()
                .map(|parameter| parameter.name)
                .collect::<Vec<_>>(),
            ["lang"]
        );
    }

    #[test]
    fn detects_complete_and_unclosed_raw_literal_ranges() {
        let complete = notist_syntax::parse("before `raw` after");
        let raw = &complete.raw_literals[0];
        assert!(is_in_raw_literal(&complete, raw.range.start));
        assert!(is_in_raw_literal(&complete, raw.payload_range.start));
        assert!(!is_in_raw_literal(&complete, raw.range.end));

        let source = "before `raw";
        let unclosed = notist_syntax::parse(source);
        assert!(is_in_raw_literal(&unclosed, source.len()));
    }

    #[test]
    fn formats_regular_and_trailing_content_signatures() {
        let registry = FunctionRegistry::with_builtins();

        assert_eq!(
            format_signature("heading", &registry.get("heading").unwrap().signature()),
            "#heading(level: Int = 1)[body: Content] -> Content"
        );
        assert_eq!(
            format_signature("quote", &registry.get("quote").unwrap().signature()),
            "#quote[body: Content] -> Content"
        );
        assert_eq!(
            format_signature("raw", &registry.get("raw").unwrap().signature()),
            "#raw(text: String, lang: String? = none) -> Content"
        );
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
        match response.response_kind {
            lsp_server::ResponseKind::Ok { result } => result,
            lsp_server::ResponseKind::Err { error } => {
                panic!("unexpected LSP error response: {}", error.message)
            }
        }
    }
}
