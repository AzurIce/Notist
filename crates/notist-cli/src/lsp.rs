use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::notification::{
    Cancel, DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    Exit, LogMessage, Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest, References, Request as _,
    Shutdown, WorkspaceSymbolRequest,
};
use lsp_types::{
    CancelParams, CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams,
    CompletionResponse, CompletionTextEdit, Diagnostic, DiagnosticSeverity,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse,
    Documentation, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents, HoverParams,
    HoverProviderCapability, InitializeParams, Location, LogMessageParams, MarkupContent,
    MarkupKind, MessageType, NumberOrString, OneOf, Position, PositionEncodingKind,
    PublishDiagnosticsParams, Range, ReferenceParams, ServerCapabilities, SymbolKind,
    TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit, Uri,
    WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use notist_analysis::{LineIndex, LineTable, discover_vault_roots};
use notist_model::TextRange;
use notist_service::protocol::ClientKind;
use notist_service::{
    CoreRequest, CoreResponse, DiagnosticRecord, OverlayDocument, PassiveDebouncedWatcher,
    ProtocolViewKind, ServiceViewId,
};

use crate::service::{LocalNotistClient, RequestHandle};
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

pub fn run(no_daemon: bool) -> Result<ExitCode, Box<dyn Error>> {
    let (connection, io_threads) = Connection::stdio();
    // Two-phase handshake: whether the client can speak our only wire
    // encoding must be known before the result is sent, so
    // `Connection::initialize` (which builds the response up front) cannot
    // be used here.
    let (initialize_id, initialize_params) = connection.initialize_start()?;
    let initialization: InitializeParams = serde_json::from_value(initialize_params)?;
    if let Err(message) = require_utf8_position_encoding(&initialization) {
        // Refuse the session instead of answering utf-8 to a client that
        // will read it as utf-16 — that would corrupt every position.
        let _ = connection.sender.send(Message::Response(Response::new_err(
            initialize_id,
            ErrorCode::RequestFailed as i32,
            message.clone(),
        )));
        eprintln!("notist lsp: {message}");
        drop(connection);
        io_threads.join()?;
        return Ok(ExitCode::FAILURE);
    }
    let initialize_result = serde_json::json!({
        "capabilities": server_capabilities(),
        "serverInfo": {
            "name": "notist",
            "version": env!("CARGO_PKG_VERSION"),
        },
    });
    connection.initialize_finish(initialize_id, initialize_result)?;
    let root = workspace_root(&initialization)?;
    let session = LspSession::new(root, no_daemon)?;
    let runtime = Runtime::spawn(&connection);
    let workspace_events = runtime.injection_sender();
    let mut watcher = PassiveDebouncedWatcher::new(Duration::from_millis(250), move |paths| {
        if !paths.is_empty() {
            let _ = workspace_events.send(SessionEvent::WorkspaceChanged);
        }
    })?;
    watcher.watch_recursive(session.root())?;

    main_loop(connection, session, runtime)?;
    drop(watcher);
    io_threads.join()?;
    Ok(ExitCode::SUCCESS)
}

/// The wire protocol is UTF-8 only: core offsets are bytes and the downlink
/// is line tables. A client that does not offer utf-8 in
/// `general.positionEncodings` is rejected at initialize instead of being
/// served positions it would read as utf-16 (a silent client defaults to
/// utf-16 per the spec, so answering utf-8 to it would corrupt everything).
fn require_utf8_position_encoding(params: &InitializeParams) -> Result<(), String> {
    let offered = params
        .capabilities
        .general
        .as_ref()
        .and_then(|general| general.position_encodings.as_ref());
    let supports_utf8 = offered.is_some_and(|list| {
        list.iter()
            .any(|candidate| candidate == &PositionEncodingKind::UTF8)
    });
    if supports_utf8 {
        Ok(())
    } else {
        Err(
            "notist lsp requires a client that offers the `utf-8` position encoding in              general.positionEncodings (VS Code languageclient >= 8, Zed, and Neovim >= 0.11              all do)"
                .into(),
        )
    }
}

fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        position_encoding: Some(PositionEncodingKind::UTF8),
        // Incremental sync: ranged edits keep client→server traffic
        // proportional to the edit, not the document. Whole-document changes
        // remain accepted for clients that still send them (the obsidian
        // plugin does today).
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                will_save: None,
                will_save_wait_until: None,
                save: Some(TextDocumentSyncSaveOptions::Supported(true)),
            },
        )),
        completion_provider: Some(CompletionOptions {
            resolve_provider: Some(false),
            trigger_characters: Some(vec![
                "[".into(),
                ":".into(),
                "#".into(),
                "(".into(),
                ",".into(),
                // ModulePath flow: `<` starts a target literal, `/` moves a
                // target into its label part and keeps import paths going.
                "<".into(),
                "/".into(),
            ]),
            ..CompletionOptions::default()
        }),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        // Obsidian-notist panels consume this experimental extension (the
        // design ruling: module-level references need a module selector, not
        // a position). Contract documented in the plugin's session header.
        experimental: Some(serde_json::json!({
            "notist": {
                "documentReferences": {
                    "directions": ["incoming", "outgoing", "both"]
                },
                "renderDocument": {}
            }
        })),
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

type PoolJob = Box<dyn FnOnce() + Send + 'static>;

/// Internal event bus. Protocol messages, filesystem wakeups, and finished
/// builds all converge here so the main loop has a single inbox; nothing
/// internal is ever injected into the client-bound outbound stream.
enum SessionEvent {
    Protocol(Message),
    Build(BuildOutcome),
    WorkspaceChanged,
    Eof,
}

/// Background workers owned by the session: one builder thread executing
/// latest-wins vault rebuilds, and a bounded pool for request handlers.
struct Runtime {
    events: mpsc::Receiver<SessionEvent>,
    inject: mpsc::Sender<SessionEvent>,
    builder: mpsc::Sender<BuildJob>,
    pool: mpsc::Sender<PoolJob>,
    handles: Vec<JoinHandle<()>>,
}

impl Runtime {
    fn injection_sender(&self) -> mpsc::Sender<SessionEvent> {
        self.inject.clone()
    }

    fn spawn(connection: &Connection) -> Self {
        let (event_tx, event_rx) = mpsc::channel::<SessionEvent>();
        let inject_tx = event_tx.clone();
        let (builder_tx, builder_rx) = mpsc::channel::<BuildJob>();
        let build_tx = event_tx.clone();
        // Forward client-bound protocol messages into the event bus and
        // signal EOF when the stream closes, so the main loop can exit even
        // while the builder still holds an event sender.
        let proto_rx = connection.receiver.clone();
        let pump = std::thread::spawn(move || {
            for message in proto_rx {
                if event_tx.send(SessionEvent::Protocol(message)).is_err() {
                    return;
                }
            }
            let _ = event_tx.send(SessionEvent::Eof);
        });
        let builder = std::thread::spawn(move || {
            while let Ok(job) = builder_rx.recv() {
                let outcome = compute_build(&job);
                if build_tx.send(SessionEvent::Build(outcome)).is_err() {
                    break;
                }
            }
        });
        let (pool_tx, pool_rx) = mpsc::channel::<PoolJob>();
        let workers = std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(2)
            .clamp(2, 8);
        // std mpsc receivers are single-consumer, so workers share one
        // receiver behind a mutex: waiting is serialized, execution is not.
        let shared_rx = Arc::new(Mutex::new(pool_rx));
        let mut handles = vec![pump, builder];
        for _ in 0..workers {
            let shared_rx = shared_rx.clone();
            handles.push(std::thread::spawn(move || {
                loop {
                    let job = shared_rx.lock().unwrap().recv();
                    match job {
                        Ok(job) => job(),
                        Err(_) => break,
                    }
                }
            }));
        }
        Self {
            events: event_rx,
            inject: inject_tx,
            builder: builder_tx,
            pool: pool_tx,
            handles,
        }
    }
}

fn main_loop(
    connection: Connection,
    session: LspSession,
    runtime: Runtime,
) -> Result<(), Box<dyn Error>> {
    let Runtime {
        events,
        builder,
        pool,
        handles,
        ..
    } = runtime;
    let result = main_loop_inner(&connection, session, &events, &builder, &pool);
    // Releasing the connection ends the stdio writer/dropper threads; it must
    // happen before `io_threads.join()` or those joins would wait forever.
    drop(connection);
    drop(builder);
    drop(pool);
    for handle in handles {
        let _ = handle.join();
    }
    result
}

fn main_loop_inner(
    connection: &Connection,
    mut session: LspSession,
    events: &mpsc::Receiver<SessionEvent>,
    builder: &mpsc::Sender<BuildJob>,
    pool: &mpsc::Sender<PoolJob>,
) -> Result<(), Box<dyn Error>> {
    // Publication failures must not take the loop down (see the notification
    // containment below): the next build republishes anyway.
    if let Err(error) = publish_diagnostics(connection, &mut session) {
        report_issue(
            connection,
            MessageType::ERROR,
            format!("diagnostics publication failed: {error}"),
        );
    }
    let cancellations: Arc<Mutex<BTreeMap<lsp_server::RequestId, Arc<AtomicBool>>>> =
        Arc::new(Mutex::new(BTreeMap::new()));
    let mut build_in_flight = false;
    loop {
        let event = match events.recv() {
            Ok(event) => event,
            Err(_) => break,
        };
        match event {
            SessionEvent::Protocol(Message::Response(_)) => {}
            SessionEvent::Eof => break,
            SessionEvent::Protocol(Message::Request(request)) => {
                if request.method.as_str() == Shutdown::METHOD {
                    // Single-consumer shutdown: answering here and awaiting
                    // `exit` on the event bus avoids racing `handle_shutdown`,
                    // whose internal receive competes with the pump thread.
                    let _ = connection
                        .sender
                        .send(Message::Response(Response::new_ok(request.id, ())));
                    continue;
                }
                let id = request.id.clone();
                let cancelled = Arc::new(AtomicBool::new(false));
                cancellations
                    .lock()
                    .unwrap()
                    .insert(id.clone(), cancelled.clone());
                let sender = connection.sender.clone();
                let context = session.request_context();
                let cancellations = cancellations.clone();
                let submit = {
                    let sender = sender.clone();
                    let id = id.clone();
                    let cancelled = cancelled.clone();
                    let cancellations = cancellations.clone();
                    Box::new(move || {
                        let response = handle_request(&context, request, &cancelled);
                        cancellations.lock().unwrap().remove(&id);
                        let response = if cancelled.load(Ordering::Acquire) {
                            Response::new_err(
                                id,
                                ErrorCode::RequestCanceled as i32,
                                "request cancelled".into(),
                            )
                        } else {
                            response
                        };
                        let _ = sender.send(Message::Response(response));
                    }) as PoolJob
                };
                if pool.send(submit).is_err() {
                    break;
                }
            }
            SessionEvent::Protocol(Message::Notification(notification)) => {
                if notification.method == Exit::METHOD {
                    break;
                }
                if notification.method == Cancel::METHOD {
                    // A malformed cancel is dropped like any other bad message:
                    // it must never be able to terminate the server.
                    let Ok(params) = serde_json::from_value::<CancelParams>(notification.params)
                    else {
                        eprintln!(
                            "notist lsp: dropped malformed {} notification",
                            notification.method
                        );
                        continue;
                    };
                    let id = match params.id {
                        NumberOrString::Number(id) => lsp_server::RequestId::from(id),
                        NumberOrString::String(id) => lsp_server::RequestId::from(id),
                    };
                    if let Some(cancelled) = cancellations.lock().unwrap().get(&id) {
                        cancelled.store(true, Ordering::Release);
                    }
                    continue;
                }
                let changed = handle_notification(connection, &mut session, notification);
                if changed {
                    session.mark_dirty();
                    session.submit_build_if_dirty(builder, &mut build_in_flight);
                }
            }
            SessionEvent::WorkspaceChanged => {
                session.mark_dirty();
                session.submit_build_if_dirty(builder, &mut build_in_flight);
            }
            SessionEvent::Build(outcome) => {
                build_in_flight = false;
                let generation = outcome.generation();
                if let BuildOutcome::Failed { error, .. } = &outcome {
                    report_issue(
                        connection,
                        MessageType::ERROR,
                        format!("workspace rebuild failed: {error}"),
                    );
                }
                session.apply_outcome(outcome);
                if generation < session.input_generation {
                    eprintln!(
                        "notist lsp: applied build for generation {generation}, inputs already at {}; rebuilding",
                        session.input_generation
                    );
                }
                if let Err(error) = publish_diagnostics(connection, &mut session) {
                    report_issue(
                        connection,
                        MessageType::ERROR,
                        format!("diagnostics publication failed: {error}"),
                    );
                }
                session.submit_build_if_dirty(builder, &mut build_in_flight);
            }
        }
    }
    Ok(())
}

fn handle_request(context: &RequestContext, request: Request, cancelled: &AtomicBool) -> Response {
    use lsp_server::ErrorCode;
    type Handled = Result<serde_json::Value, (ErrorCode, String)>;

    let id = request.id.clone();
    // Parameter decoding failures are client mistakes (`InvalidParams`);
    // everything after a successful decode is a server-side condition
    // (`InternalError`), so clients can retry bad input without mistaking
    // server faults for their own.
    let handled: Handled = match request.method.as_str() {
        GotoDefinition::METHOD => parse_params(request.params)
            .map_err(|message| (ErrorCode::InvalidParams, message))
            .and_then(|params: GotoDefinitionParams| {
                definition(context, params, cancelled)
                    .map_err(|message| (ErrorCode::InternalError, message))
                    .and_then(|value| {
                        to_json(value)
                            .map_err(|error| (ErrorCode::InternalError, error.to_string()))
                    })
            }),
        References::METHOD => parse_params(request.params)
            .map_err(|message| (ErrorCode::InvalidParams, message))
            .and_then(|params: ReferenceParams| {
                references(context, params, cancelled)
                    .map_err(|message| (ErrorCode::InternalError, message))
                    .and_then(|value| {
                        to_json(value)
                            .map_err(|error| (ErrorCode::InternalError, error.to_string()))
                    })
            }),
        Completion::METHOD => parse_params(request.params)
            .map_err(|message| (ErrorCode::InvalidParams, message))
            .and_then(|params: CompletionParams| {
                completion(context, params, cancelled)
                    .map_err(|message| (ErrorCode::InternalError, message))
                    .and_then(|value| {
                        to_json(value)
                            .map_err(|error| (ErrorCode::InternalError, error.to_string()))
                    })
            }),
        HoverRequest::METHOD => parse_params(request.params)
            .map_err(|message| (ErrorCode::InvalidParams, message))
            .and_then(|params: HoverParams| {
                hover(context, params, cancelled)
                    .map_err(|message| (ErrorCode::InternalError, message))
                    .and_then(|value| {
                        to_json(value)
                            .map_err(|error| (ErrorCode::InternalError, error.to_string()))
                    })
            }),
        DocumentSymbolRequest::METHOD => parse_params(request.params)
            .map_err(|message| (ErrorCode::InvalidParams, message))
            .and_then(|params: DocumentSymbolParams| {
                document_symbols(context, params, cancelled)
                    .map_err(|message| (ErrorCode::InternalError, message))
                    .and_then(|value| {
                        to_json(value)
                            .map_err(|error| (ErrorCode::InternalError, error.to_string()))
                    })
            }),
        WorkspaceSymbolRequest::METHOD => parse_params(request.params)
            .map_err(|message| (ErrorCode::InvalidParams, message))
            .and_then(|params: WorkspaceSymbolParams| {
                workspace_symbols(context, params, cancelled)
                    .map_err(|message| (ErrorCode::InternalError, message))
                    .and_then(|value| {
                        to_json(value)
                            .map_err(|error| (ErrorCode::InternalError, error.to_string()))
                    })
            }),
        DOCUMENT_REFERENCES_METHOD => parse_params(request.params)
            .map_err(|message| (ErrorCode::InvalidParams, message))
            .and_then(|params: DocumentReferencesParams| {
                document_references(context, params, cancelled)
                    .map_err(|message| (ErrorCode::InternalError, message))
                    .and_then(|value| {
                        to_json(value)
                            .map_err(|error| (ErrorCode::InternalError, error.to_string()))
                    })
            }),
        RENDER_DOCUMENT_METHOD => parse_params(request.params)
            .map_err(|message| (ErrorCode::InvalidParams, message))
            .and_then(|params: RenderDocumentParams| {
                render_document(context, params, cancelled)
                    .map_err(|message| (ErrorCode::InternalError, message))
                    .and_then(|value| {
                        to_json(value)
                            .map_err(|error| (ErrorCode::InternalError, error.to_string()))
                    })
            }),
        _ => {
            return Response::new_err(
                id,
                ErrorCode::MethodNotFound as i32,
                format!("unsupported request `{}`", request.method),
            );
        }
    };

    match handled {
        Ok(value) => Response::new_ok(id, value),
        Err((code, message)) => Response::new_err(id, code as i32, message),
    }
}

fn parse_params<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|error| error.to_string())
}

fn to_json<T: serde::Serialize>(value: T) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|error| error.to_string())
}

/// Reports a server-side condition on both channels the client can actually
/// see: stderr for host logs and `window/logMessage` for LSP clients.
/// Protocol violations and rebuild failures used to be stderr-only, which
/// left clients wondering why diagnostics went stale.
fn report_issue(connection: &Connection, level: MessageType, message: String) {
    eprintln!("notist lsp: {message}");
    let _ = connection
        .sender
        .send(Message::Notification(Notification::new(
            LogMessage::METHOD.into(),
            LogMessageParams {
                typ: level,
                message,
            },
        )));
}

/// Failure classes for one client notification. Decode failures are pure
/// client bugs (stderr only); session-level failures (unroutable URIs,
/// contract issues) also reach the client through `window/logMessage`.
enum NotificationError {
    Decode(serde_json::Error),
    Session(Box<dyn Error>),
}

/// Handles one client notification. Every failure is contained here — a
/// malformed or unroutable message must never take the server down — and the
/// return value only reports whether vault inputs changed.
fn handle_notification(
    connection: &Connection,
    session: &mut LspSession,
    notification: Notification,
) -> bool {
    let method = notification.method.clone();
    let handled = match notification.method.as_str() {
        DidOpenTextDocument::METHOD => decode_notification(notification.params, |params| {
            session.open(params).map(|_| true)
        }),
        DidChangeTextDocument::METHOD => {
            decode_notification(notification.params, |params| session.change(params))
        }
        DidSaveTextDocument::METHOD => {
            decode_notification(notification.params, |params| session.save(params))
        }
        DidCloseTextDocument::METHOD => decode_notification(notification.params, |params| {
            session.close(params).map(|_| true)
        }),
        _ => Ok(false),
    };
    match handled {
        Ok(changed) => changed,
        Err(NotificationError::Decode(error)) => {
            eprintln!("notist lsp: dropped malformed {method} notification: {error}");
            false
        }
        Err(NotificationError::Session(error)) => {
            report_issue(
                connection,
                MessageType::WARNING,
                format!("rejected {method}: {error}"),
            );
            false
        }
    }
}

fn decode_notification<T: serde::de::DeserializeOwned>(
    params: serde_json::Value,
    handle: impl FnOnce(T) -> Result<bool, Box<dyn Error>>,
) -> Result<bool, NotificationError> {
    let params: T = serde_json::from_value(params).map_err(NotificationError::Decode)?;
    handle(params).map_err(NotificationError::Session)
}

#[derive(Clone, Debug)]
struct OpenDocument {
    version: i32,
    source: Arc<str>,
}

/// The adapter's coordinate converter for one source, backed by the file's
/// byte-offset line table. The wire protocol is UTF-8 only, so columns are
/// byte columns and convert without the source text; positions past line
/// boundaries saturate (core validates offsets like it always has for CLI
/// callers).
#[derive(Clone)]
struct ClientSource {
    table: Arc<LineTable>,
}

impl ClientSource {
    fn from_text(text: &str) -> Self {
        Self {
            table: Arc::new(LineIndex::new(text).line_table(text)),
        }
    }

    fn from_parts(starts: Vec<u32>, content_ends: Vec<u32>, end: u32) -> Self {
        Self {
            table: Arc::new(LineTable {
                starts: starts.into(),
                content_ends: content_ends.into(),
                end,
            }),
        }
    }

    /// Incoming LSP position → byte offset.
    fn offset(&self, position: Position) -> usize {
        self.table.offset(position.line, position.character)
    }

    /// Outgoing byte offset → LSP position.
    fn position(&self, offset: usize) -> Position {
        let (line, character) = self.table.position(offset);
        Position::new(line, character)
    }

    fn range(&self, range: TextRange) -> Range {
        Range::new(self.position(range.start), self.position(range.end))
    }
}

/// Shared per-vault request entry point; embedded services serve concurrent
/// queries directly while daemon connections remain serialized internally.
type VaultClient = RequestHandle;

/// Main-thread-owned session state; the event loop is the single mutator.
struct LspSession {
    root: PathBuf,
    no_daemon: bool,
    documents: BTreeMap<PathBuf, OpenDocument>,
    vaults: BTreeMap<PathBuf, VaultSession>,
    published_paths: BTreeSet<PathBuf>,
    /// Content signature of the diagnostics last sent per path; unchanged
    /// sets are not republished (delta publication).
    published_signatures: BTreeMap<PathBuf, u64>,
    /// Document versions the service's overlay set is known to hold. Drives
    /// the UpdateViewDelta diff: only changed documents travel the wire.
    sent_overlays: BTreeMap<PathBuf, i32>,
    input_generation: u64,
    dirty: bool,
}

struct VaultSession {
    view_id: ServiceViewId,
    client: VaultClient,
    sources: Arc<BTreeMap<PathBuf, ClientSource>>,
}

impl VaultSession {
    fn request(&self, request: CoreRequest) -> Result<notist_service::CoreReply, Box<dyn Error>> {
        Ok(self.client.request(request)?)
    }
}

/// Cheap immutable capture handed to request workers: Arc handles only, no
/// document bodies. A concurrent build swaps `sources` wholesale, so workers
/// keep serving from the snapshot they captured.
#[derive(Clone)]
struct QueryVault {
    view_id: ServiceViewId,
    client: VaultClient,
    sources: Arc<BTreeMap<PathBuf, ClientSource>>,
}

impl QueryVault {
    /// Cancels cooperatively on the embedded path; daemon requests observe
    /// cancellation only after completion.
    fn cancellable(
        &self,
        request: CoreRequest,
        cancelled: &AtomicBool,
    ) -> Result<notist_service::CoreReply, Box<dyn Error>> {
        Ok(self.client.cancellable(request, cancelled)?)
    }
}

#[derive(Clone)]
struct RequestContext {
    vaults: BTreeMap<PathBuf, QueryVault>,
}

/// One unit of latest-wins build work, captured from the session on the main
/// thread and executed on the builder thread.
struct BuildJob {
    workspace_root: PathBuf,
    no_daemon: bool,
    documents: BTreeMap<PathBuf, OpenDocument>,
    live_vaults: Vec<(PathBuf, ServiceViewId, VaultClient)>,
    generation: u64,
    /// Overlays that differ from `sent_overlays`, plus overlays the service
    /// still holds for documents that have since been closed.
    overlay_upsert: Vec<OverlayDocument>,
    overlay_remove: Vec<PathBuf>,
    /// The overlay versions the service will hold once this job applies.
    new_sent: BTreeMap<PathBuf, i32>,
}

struct BuiltVault {
    root: PathBuf,
    view_id: ServiceViewId,
    client: VaultClient,
    sources: Arc<BTreeMap<PathBuf, ClientSource>>,
}

enum BuildOutcome {
    Applied {
        generation: u64,
        vaults: Vec<BuiltVault>,
        /// Overlay versions the service now holds; committed only on apply.
        new_sent: BTreeMap<PathBuf, i32>,
    },
    Failed {
        generation: u64,
        error: String,
    },
}

impl BuildOutcome {
    fn generation(&self) -> u64 {
        match self {
            BuildOutcome::Applied { generation, .. } => *generation,
            BuildOutcome::Failed { generation, .. } => *generation,
        }
    }
}

fn compute_build(job: &BuildJob) -> BuildOutcome {
    let generation = job.generation;
    match compute_build_inner(job) {
        Ok(vaults) => BuildOutcome::Applied {
            generation,
            vaults,
            new_sent: job.new_sent.clone(),
        },
        Err(error) => BuildOutcome::Failed {
            generation,
            error: error.to_string(),
        },
    }
}

fn compute_build_inner(job: &BuildJob) -> Result<Vec<BuiltVault>, Box<dyn Error>> {
    let mut roots = discover_vault_roots(&job.workspace_root)?;
    if roots.is_empty() {
        roots.push(job.workspace_root.clone());
    }
    let mut previous: BTreeMap<PathBuf, (ServiceViewId, VaultClient)> = job
        .live_vaults
        .iter()
        .map(|(root, view_id, client)| (root.clone(), (*view_id, client.clone())))
        .collect();
    let mut built = Vec::new();
    for root in &roots {
        let (view_id, client) = if let Some(handle) = previous.remove(root) {
            handle
        } else {
            let handle = LocalNotistClient::connect(job.no_daemon, ClientKind::Lsp, root.clone())?
                .into_request_handle();
            let reply = handle.request(CoreRequest::OpenView {
                root: root.clone(),
                kind: ProtocolViewKind::Session,
            })?;
            let CoreResponse::Opened { view_id, .. } = reply.response else {
                return Err("service returned an unexpected open-view response".into());
            };
            (view_id, handle)
        };
        // Overlay sync: a burst that changed overlays travels as a delta; a
        // watcher-only build (overlays untouched) keeps the full update so
        // its disk-refresh semantics are preserved exactly.
        let upsert: Vec<_> = job
            .overlay_upsert
            .iter()
            .filter(|document| assigned_vault_root(&document.path, &roots) == Some(root))
            .cloned()
            .collect();
        let remove: Vec<_> = job
            .overlay_remove
            .iter()
            .filter(|path| assigned_vault_root(path, &roots) == Some(root))
            .cloned()
            .collect();
        if !upsert.is_empty() || !remove.is_empty() {
            client.request(CoreRequest::UpdateViewDelta {
                view_id,
                upsert,
                remove,
            })?;
        } else {
            // Watcher-only build: nothing changed on the overlay side, but the
            // full update is still what pulls external disk edits into the
            // session snapshot — and it must carry the COMPLETE overlay set
            // (replace semantics), never an empty one.
            let documents: Vec<_> = job
                .documents
                .iter()
                .filter(|(path, _)| assigned_vault_root(path, &roots) == Some(root))
                .map(|(path, document)| OverlayDocument {
                    path: path.clone(),
                    version: i64::from(document.version),
                    text: document.source.to_string(),
                })
                .collect();
            client.request(CoreRequest::UpdateView {
                view_id,
                documents,
                configuration: None,
            })?;
        }
        // Downlink: per-file line tables — everything the adapter needs to
        // convert byte columns, without pulling any source text.
        let tables_reply = client.request(CoreRequest::SourceTables { view_id })?;
        let CoreResponse::SourceTables(tables) = tables_reply.response else {
            return Err("service returned an unexpected source-tables response".into());
        };
        let sources = tables
            .into_iter()
            .map(|record| {
                (
                    record.path,
                    ClientSource::from_parts(record.line_starts, record.content_ends, record.end),
                )
            })
            .collect::<BTreeMap<_, _>>();
        built.push(BuiltVault {
            root: root.clone(),
            view_id,
            client,
            sources: Arc::new(sources),
        });
    }
    for (root, (view_id, client)) in previous {
        if let Err(error) = client.request(CoreRequest::CloseView { view_id }) {
            eprintln!(
                "notist lsp: failed to close stale view {view_id:?} for {}: {error}",
                root.display()
            );
        }
    }
    Ok(built)
}

impl LspSession {
    fn new(root: PathBuf, no_daemon: bool) -> Result<Self, Box<dyn Error>> {
        let mut session = Self {
            root,
            no_daemon,
            documents: BTreeMap::new(),
            vaults: BTreeMap::new(),
            published_paths: BTreeSet::new(),
            published_signatures: BTreeMap::new(),
            sent_overlays: BTreeMap::new(),
            input_generation: 0,
            dirty: false,
        };
        session.rebuild_blocking()?;
        Ok(session)
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn mark_dirty(&mut self) {
        self.input_generation += 1;
        self.dirty = true;
    }

    fn submit_build_if_dirty(&mut self, builder: &mpsc::Sender<BuildJob>, in_flight: &mut bool) {
        if !*in_flight && self.dirty {
            *in_flight = true;
            self.dirty = false;
            let job = self.build_job();
            if builder.send(job).is_err() {
                *in_flight = false;
            }
        }
    }

    fn build_job(&self) -> BuildJob {
        BuildJob {
            workspace_root: self.root.clone(),
            no_daemon: self.no_daemon,
            documents: self.documents.clone(),
            live_vaults: self
                .vaults
                .iter()
                .map(|(root, vault)| (root.clone(), vault.view_id, vault.client.clone()))
                .collect(),
            generation: self.input_generation,
            overlay_upsert: self
                .documents
                .iter()
                .filter(|(path, document)| self.sent_overlays.get(*path) != Some(&document.version))
                .map(|(path, document)| OverlayDocument {
                    path: path.clone(),
                    version: i64::from(document.version),
                    text: document.source.to_string(),
                })
                .collect(),
            overlay_remove: self
                .sent_overlays
                .keys()
                .filter(|path| !self.documents.contains_key(*path))
                .cloned()
                .collect(),
            new_sent: self
                .documents
                .iter()
                .map(|(path, document)| (path.clone(), document.version))
                .collect(),
        }
    }

    /// Runs one build synchronously on the calling thread, propagating
    /// failures; used for the initial session build and by tests.
    fn rebuild_blocking(&mut self) -> Result<(), Box<dyn Error>> {
        self.mark_dirty();
        let job = self.build_job();
        let vaults = compute_build_inner(&job)?;
        self.sent_overlays = job.new_sent;
        self.vaults = vaults
            .into_iter()
            .map(|built| {
                (
                    built.root,
                    VaultSession {
                        view_id: built.view_id,
                        client: built.client,
                        sources: built.sources,
                    },
                )
            })
            .collect();
        self.dirty = false;
        Ok(())
    }

    fn apply_outcome(&mut self, outcome: BuildOutcome) {
        match outcome {
            BuildOutcome::Applied {
                vaults, new_sent, ..
            } => {
                self.sent_overlays = new_sent;
                self.vaults = vaults
                    .into_iter()
                    .map(|built| {
                        (
                            built.root,
                            VaultSession {
                                view_id: built.view_id,
                                client: built.client,
                                sources: built.sources,
                            },
                        )
                    })
                    .collect();
            }
            // Failure reporting happens in the main loop, which owns the
            // client connection. The overlay bookkeeping cannot be trusted
            // for a failed build: clear it so the next build resends every
            // overlay (self-healing).
            BuildOutcome::Failed { .. } => self.sent_overlays.clear(),
        }
    }

    fn request_context(&self) -> RequestContext {
        RequestContext {
            vaults: self
                .vaults
                .iter()
                .map(|(root, vault)| {
                    (
                        root.clone(),
                        QueryVault {
                            view_id: vault.view_id,
                            client: vault.client.clone(),
                            sources: vault.sources.clone(),
                        },
                    )
                })
                .collect(),
        }
    }

    fn open(&mut self, params: DidOpenTextDocumentParams) -> Result<(), Box<dyn Error>> {
        let path = normalize_uri_path(&params.text_document.uri)?;
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
        let path = normalize_uri_path(&params.text_document.uri)?;
        let Some(document) = self.documents.get_mut(&path) else {
            // Clients must open a document before changing it; a ranged change
            // against an unknown buffer would silently corrupt text, so the
            // notification is dropped rather than guessed at.
            eprintln!(
                "notist lsp: unexpected didChange for unopened document `{}`",
                path.display()
            );
            return Ok(false);
        };
        let new_text = apply_document_changes(&document.source, &params.content_changes);
        let mut changed = false;
        if new_text != document.source.as_ref() {
            document.source = Arc::from(new_text.as_str());
            changed = true;
        }
        document.version = params.text_document.version;
        Ok(changed)
    }

    fn save(&mut self, params: DidSaveTextDocumentParams) -> Result<bool, Box<dyn Error>> {
        let path = normalize_uri_path(&params.text_document.uri)?;
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
        let path = normalize_uri_path(&params.text_document.uri)?;
        self.documents.remove(&path);
        Ok(())
    }

    fn document_version(&self, path: &Path) -> Option<i32> {
        self.documents.get(path).map(|document| document.version)
    }
}

fn workspace_for_source<'a>(context: &'a RequestContext, path: &Path) -> Option<&'a QueryVault> {
    let root = assigned_vault_root(path, context.vaults.keys())?;
    context.vaults.get(root)
}

#[allow(deprecated)]
fn document_symbols(
    context: &RequestContext,
    params: DocumentSymbolParams,
    cancelled: &AtomicBool,
) -> Result<Option<DocumentSymbolResponse>, String> {
    let path = normalize_uri_path(&params.text_document.uri).map_err(|error| error.to_string())?;
    let Some(workspace) = workspace_for_source(context, &path) else {
        return Ok(None);
    };
    let Some(source) = workspace.sources.get(&path) else {
        return Ok(Some(DocumentSymbolResponse::Nested(Vec::new())));
    };
    let reply = workspace
        .cancellable(
            CoreRequest::DocumentSymbols {
                view_id: workspace.view_id,
                path: path.clone(),
            },
            cancelled,
        )
        .map_err(|error| error.to_string())?;
    let CoreResponse::DocumentSymbols(symbols) = reply.response else {
        return Err("service returned an unexpected document-symbol response".into());
    };
    let symbols = symbols
        .into_iter()
        .map(|symbol| {
            let range = lsp_range(source, symbol.range.into());
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
    context: &RequestContext,
    params: WorkspaceSymbolParams,
    cancelled: &AtomicBool,
) -> Result<Option<WorkspaceSymbolResponse>, String> {
    let mut symbols = Vec::new();
    for view in context.vaults.values() {
        let reply = view
            .cancellable(
                CoreRequest::WorkspaceSymbols {
                    view_id: view.view_id,
                    query: params.query.clone(),
                },
                cancelled,
            )
            .map_err(|error| error.to_string())?;
        let CoreResponse::WorkspaceSymbols(workspace_symbols) = reply.response else {
            return Err("service returned an unexpected workspace-symbol response".into());
        };
        for symbol in workspace_symbols {
            let Some(source) = view.sources.get(&symbol.path) else {
                continue;
            };
            let uri = file_path_to_uri(&symbol.path)?;
            symbols.push(lsp_types::SymbolInformation {
                name: symbol.name,
                kind: match symbol.kind.as_str() {
                    "annotation" => SymbolKind::KEY,
                    _ => SymbolKind::FILE,
                },
                tags: None,
                deprecated: None,
                location: Location::new(uri, lsp_range(source, symbol.range.into())),
                container_name: Some("Notist vault".into()),
            });
        }
    }
    symbols.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(Some(WorkspaceSymbolResponse::Flat(symbols)))
}

/// Experimental method name for module-level document references; declared
/// under `capabilities.experimental.notist.documentReferences`.
const DOCUMENT_REFERENCES_METHOD: &str = "notist/documentReferences";

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentReferencesParams {
    text_document: TextDocumentIdentifier,
    /// "incoming" | "outgoing" | "both"; defaults to incoming.
    direction: Option<String>,
    #[serde(default)]
    include_definition: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LspDocumentReferenceItem {
    uri: Uri,
    range: Range,
    direction: String,
    source_module: String,
    target_module: String,
    target_name: Option<String>,
    target_kind: Option<String>,
    url: Option<String>,
    is_definition: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LspDocumentReferencesResult {
    revision: u64,
    items: Vec<LspDocumentReferenceItem>,
}

/// Experimental method name for the single-document render consumed by the
/// obsidian-notist preview view; declared under
/// `capabilities.experimental.notist.renderDocument`.
const RENDER_DOCUMENT_METHOD: &str = "notist/renderDocument";

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenderDocumentParams {
    text_document: TextDocumentIdentifier,
}

/// The service's page record verbatim (snake_case fields on the wire, same
/// shapes RenderWorkspace produces) plus the snapshot revision and the
/// module's resource table for fragment URL rewriting.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LspRenderDocumentResult {
    revision: u64,
    page: notist_service::RenderedPageRecord,
    resources: Vec<notist_service::RenderedResourceRecord>,
}

/// Module-level references by document identity — no position selector, so a
/// document that opens with a heading resolves to its owning module, not to
/// the heading symbol (the `textDocument/references` ambiguity).
fn document_references(
    context: &RequestContext,
    params: DocumentReferencesParams,
    cancelled: &AtomicBool,
) -> Result<Option<LspDocumentReferencesResult>, String> {
    let path = normalize_uri_path(&params.text_document.uri).map_err(|error| error.to_string())?;
    let Some(workspace) = workspace_for_source(context, &path) else {
        return Ok(None);
    };
    let direction = match params.direction.as_deref() {
        Some("outgoing") => notist_service::query::ReferenceDirection::Outgoing,
        Some("both") => notist_service::query::ReferenceDirection::Both,
        _ => notist_service::query::ReferenceDirection::Incoming,
    };
    let reply = workspace
        .cancellable(
            CoreRequest::DocumentReferences {
                view_id: workspace.view_id,
                path: path.clone(),
                direction,
                include_definition: params.include_definition,
            },
            cancelled,
        )
        .map_err(|error| error.to_string())?;
    let CoreResponse::DocumentReferences(result) = reply.response else {
        return Err("service returned an unexpected document-references response".into());
    };
    let mut items = Vec::with_capacity(result.items.len());
    for item in result.items {
        let Some(source) = workspace.sources.get(&item.path) else {
            continue;
        };
        let uri = file_path_to_uri(&item.path)?;
        items.push(LspDocumentReferenceItem {
            uri,
            range: lsp_range(source, item.range.into()),
            direction: item.direction,
            source_module: item.source_module,
            target_module: item.target_module,
            target_name: item.target_name,
            target_kind: item.target_kind,
            url: item.url,
            is_definition: item.is_definition,
        });
    }
    Ok(Some(LspDocumentReferencesResult {
        revision: result.revision,
        items,
    }))
}

/// Rendered HTML fragment for the document's owning module, from the same
/// evaluated pipeline the preview site uses. Null when the file belongs to
/// no assigned vault or is not a `.not` source (resources have no module).
fn render_document(
    context: &RequestContext,
    params: RenderDocumentParams,
    cancelled: &AtomicBool,
) -> Result<Option<LspRenderDocumentResult>, String> {
    let path = normalize_uri_path(&params.text_document.uri).map_err(|error| error.to_string())?;
    if !path.extension().is_some_and(|extension| extension == "not") {
        return Ok(None);
    }
    let Some(workspace) = workspace_for_source(context, &path) else {
        return Ok(None);
    };
    let reply = workspace
        .cancellable(
            CoreRequest::RenderDocument {
                view_id: workspace.view_id,
                path: path.clone(),
            },
            cancelled,
        )
        .map_err(|error| error.to_string())?;
    let CoreResponse::RenderedDocument(record) = reply.response else {
        return Err("service returned an unexpected render-document response".into());
    };
    Ok(Some(LspRenderDocumentResult {
        revision: reply.snapshot.revision,
        page: record.page,
        resources: record.resources,
    }))
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

/// Applies LSP content changes sequentially, mirroring rust-analyzer's
/// `apply_document_changes`. A whole-document change (no range) becomes the
/// base text and discards all preceding partial changes — equivalent to pure
/// sequential semantics, since a later whole-document change overwrites
/// everything before it. Partial changes apply in order against the evolving
/// text; the line index is rebuilt only when a change reaches past the still
/// valid prefix, so descending edit batches (the common client shape) rebuild
/// nothing. Out-of-bounds positions clamp into the document, and a change
/// whose clamped start exceeds its clamped end is skipped.
fn apply_document_changes(
    file_contents: &str,
    content_changes: &[TextDocumentContentChangeEvent],
) -> String {
    let (mut text, partials) = match content_changes
        .iter()
        .rposition(|change| change.range.is_none())
    {
        Some(base) => (
            content_changes[base].text.clone(),
            &content_changes[base + 1..],
        ),
        None => (file_contents.to_owned(), content_changes),
    };
    if partials.is_empty() {
        return text;
    }

    let mut index = LineIndex::new(&text);
    // Lines up to `index_valid` are unchanged since `index` was built.
    let mut index_valid = u32::MAX;
    for change in partials {
        let Some(range) = &change.range else { continue };
        if index_valid <= range.end.line {
            index = LineIndex::new(&text);
        }
        index_valid = range.start.line;
        let start = index.offset_utf8_saturating(&text, range.start.line, range.start.character);
        let end = index.offset_utf8_saturating(&text, range.end.line, range.end.character);
        if start <= end {
            text.replace_range(start..end, &change.text);
        }
    }
    text
}

fn normalize_uri_path(uri: &Uri) -> Result<PathBuf, Box<dyn Error>> {
    let path = uri_to_file_path(uri)?;
    if path.exists() {
        return Ok(dunce::canonicalize(path)?);
    }
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("document path `{}` has no file name", path.display()))?;
    let mut missing = vec![file_name.to_owned()];
    let mut ancestor = path
        .parent()
        .ok_or_else(|| format!("document path `{}` has no parent", path.display()))?;
    while !ancestor.exists() {
        let name = ancestor.file_name().ok_or_else(|| {
            format!(
                "document path `{}` does not live inside an existing directory",
                path.display()
            )
        })?;
        missing.push(name.to_owned());
        ancestor = ancestor.parent().ok_or_else(|| {
            format!(
                "document path `{}` escapes the filesystem root",
                path.display()
            )
        })?;
    }
    let mut normalized = dunce::canonicalize(ancestor)?;
    for name in missing.into_iter().rev() {
        normalized.push(name);
    }
    Ok(normalized)
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

fn diagnostics_signature(diagnostics: &[Diagnostic]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for diagnostic in diagnostics {
        diagnostic.range.start.line.hash(&mut hasher);
        diagnostic.range.start.character.hash(&mut hasher);
        diagnostic.range.end.line.hash(&mut hasher);
        diagnostic.range.end.character.hash(&mut hasher);
        diagnostic
            .severity
            .map(|severity| format!("{severity:?}"))
            .hash(&mut hasher);
        diagnostic.message.hash(&mut hasher);
        if let Some(code) = &diagnostic.code {
            code.hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// Publishes only paths whose diagnostics actually changed since the last
/// cycle; vanished paths receive one empty set to clear client state.
fn publish_diagnostics(
    connection: &Connection,
    session: &mut LspSession,
) -> Result<(), Box<dyn Error>> {
    let mut diagnostics_by_path: BTreeMap<PathBuf, Vec<Diagnostic>> = BTreeMap::new();
    for view in session.vaults.values() {
        let reply = view.request(CoreRequest::Diagnostics {
            view_id: view.view_id,
        })?;
        let CoreResponse::Diagnostics(records) = reply.response else {
            return Err("service returned an unexpected diagnostics response".into());
        };
        for record in records {
            let Some(path) = record.path.clone() else {
                continue;
            };
            let source = view.sources.get(&path);
            diagnostics_by_path
                .entry(path)
                .or_default()
                .push(lsp_diagnostic(source, record));
        }
    }

    let current_paths: BTreeSet<_> = session
        .vaults
        .values()
        .flat_map(|view| view.sources.keys().cloned())
        .collect();
    let paths: BTreeSet<_> = current_paths
        .union(&session.published_paths)
        .cloned()
        .collect();
    let mut signatures = std::mem::take(&mut session.published_signatures);
    for path in paths {
        let diagnostics = diagnostics_by_path.get(&path).cloned().unwrap_or_default();
        let signature = diagnostics_signature(&diagnostics);
        if signatures.get(&path) == Some(&signature) {
            continue;
        }
        let uri = file_path_to_uri(&path)?;
        let params =
            PublishDiagnosticsParams::new(uri, diagnostics, session.document_version(&path));
        connection
            .sender
            .send(Message::Notification(Notification::new(
                PublishDiagnostics::METHOD.to_owned(),
                params,
            )))?;
        signatures.insert(path, signature);
    }
    session.published_signatures = signatures;
    session.published_paths = current_paths;
    Ok(())
}

fn lsp_severity(severity: &str) -> DiagnosticSeverity {
    match severity {
        "warning" => DiagnosticSeverity::WARNING,
        "info" | "hint" => DiagnosticSeverity::INFORMATION,
        _ => DiagnosticSeverity::ERROR,
    }
}

fn lsp_diagnostic(source: Option<&ClientSource>, diagnostic: DiagnosticRecord) -> Diagnostic {
    let captured_source = diagnostic
        .source
        .clone()
        .map(|text| ClientSource::from_text(&text));
    let source = captured_source.as_ref().or(source);
    let range = diagnostic
        .range
        .map(Into::into)
        .unwrap_or(TextRange::new(0, 0));
    Diagnostic {
        range: source.map_or_else(
            || Range::new(Position::new(0, 0), Position::new(0, 0)),
            |source| lsp_range(source, range),
        ),
        severity: Some(lsp_severity(&diagnostic.severity)),
        code: Some(NumberOrString::String(diagnostic.code)),
        source: Some("notist".into()),
        message: diagnostic.message,
        ..Diagnostic::default()
    }
}

fn definition(
    context: &RequestContext,
    params: GotoDefinitionParams,
    cancelled: &AtomicBool,
) -> Result<Option<GotoDefinitionResponse>, String> {
    let position = params.text_document_position_params;
    let Some((path, workspace, _source, offset)) = source_position(context, &position) else {
        return Ok(None);
    };
    let reply = workspace
        .cancellable(
            CoreRequest::Definition {
                view_id: workspace.view_id,
                path,
                offset,
            },
            cancelled,
        )
        .map_err(|error| error.to_string())?;
    let CoreResponse::Definition(definition) = reply.response else {
        return Err("service returned an unexpected definition response".into());
    };
    let Some(definition) = definition else {
        return Ok(None);
    };
    let target_source = ClientSource::from_text(&definition.source);
    let uri = file_path_to_uri(&definition.path)?;
    Ok(Some(GotoDefinitionResponse::Scalar(Location::new(
        uri,
        lsp_range(&target_source, definition.range.into()),
    ))))
}

fn references(
    context: &RequestContext,
    params: ReferenceParams,
    cancelled: &AtomicBool,
) -> Result<Option<Vec<Location>>, String> {
    let position = params.text_document_position;
    let Some((path, workspace, _source, offset)) = source_position(context, &position) else {
        return Ok(None);
    };
    let reply = workspace
        .cancellable(
            CoreRequest::References {
                view_id: workspace.view_id,
                path,
                offset,
                include_definition: params.context.include_declaration,
            },
            cancelled,
        )
        .map_err(|error| error.to_string())?;
    let CoreResponse::References(results) = reply.response else {
        return Err("service returned an unexpected references response".into());
    };
    let mut locations = Vec::new();
    for result in results {
        let source = ClientSource::from_text(&result.source);
        let uri = file_path_to_uri(&result.path)?;
        locations.push(Location::new(uri, lsp_range(&source, result.range.into())));
    }
    Ok((!locations.is_empty()).then_some(locations))
}

fn completion(
    context: &RequestContext,
    params: CompletionParams,
    cancelled: &AtomicBool,
) -> Result<Option<CompletionResponse>, String> {
    let position = params.text_document_position;
    let Some((path, workspace, source_input, offset)) = source_position(context, &position) else {
        return Ok(None);
    };
    let reply = workspace
        .cancellable(
            CoreRequest::Completion {
                view_id: workspace.view_id,
                path,
                offset,
            },
            cancelled,
        )
        .map_err(|error| error.to_string())?;
    let CoreResponse::Completion(candidates) = reply.response else {
        return Err("service returned an unexpected completion response".into());
    };
    let items = candidates
        .into_iter()
        .map(|candidate| CompletionItem {
            label: candidate.label,
            kind: Some(match candidate.kind.as_str() {
                "module" => CompletionItemKind::MODULE,
                "function" => CompletionItemKind::FUNCTION,
                "parameter" => CompletionItemKind::FIELD,
                _ => CompletionItemKind::PROPERTY,
            }),
            detail: Some(candidate.detail),
            documentation: candidate.documentation.map(Documentation::String),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range: lsp_range(source_input, candidate.replacement.into()),
                new_text: candidate.insert_text,
            })),
            ..CompletionItem::default()
        })
        .collect::<Vec<_>>();
    Ok((!items.is_empty()).then_some(CompletionResponse::Array(items)))
}

fn hover(
    context: &RequestContext,
    params: HoverParams,
    cancelled: &AtomicBool,
) -> Result<Option<Hover>, String> {
    let position = params.text_document_position_params;
    let Some((path, workspace, source_input, offset)) = source_position(context, &position) else {
        return Ok(None);
    };
    let reply = workspace
        .cancellable(
            CoreRequest::Hover {
                view_id: workspace.view_id,
                path,
                offset,
            },
            cancelled,
        )
        .map_err(|error| error.to_string())?;
    let CoreResponse::Hover(hover) = reply.response else {
        return Err("service returned an unexpected hover response".into());
    };
    let Some(hover) = hover else {
        return Ok(None);
    };
    Ok(Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: hover.markdown,
        }),
        range: Some(lsp_range(source_input, hover.range.into())),
    }))
}

fn source_position<'a>(
    context: &'a RequestContext,
    params: &lsp_types::TextDocumentPositionParams,
) -> Option<(PathBuf, &'a QueryVault, &'a ClientSource, usize)> {
    let path = normalize_uri_path(&params.text_document.uri).ok()?;
    let workspace = workspace_for_source(context, &path)?;
    let source = workspace.sources.get(&path)?;
    let offset = source.offset(params.position);
    Some((path, workspace, source, offset))
}

fn lsp_range(source: &ClientSource, range: TextRange) -> Range {
    source.range(range)
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
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    #[test]
    fn converts_utf8_offsets_and_positions() {
        let source = "a😀中\r\nnext";
        let index = LineIndex::new(source);

        // UTF-8 columns are plain byte offsets within the line.
        assert_eq!(index.utf8_position(source, "a😀中".len()), Some((0, 8)));
        assert_eq!(index.utf8_position(source, "a😀中\r\n".len()), Some((1, 0)));
        assert_eq!(index.utf8_position(source, source.len()), Some((1, 4)));
        assert_eq!(index.offset_utf8(source, 0, 8), Some("a😀中".len()));
        assert_eq!(index.offset_utf8(source, 1, 2), Some("a😀中\r\nne".len()));
        // Column lands inside a character, or past the line end.
        assert_eq!(index.offset_utf8(source, 0, 2), None);
        assert_eq!(index.offset_utf8(source, 1, 5), None);
    }

    #[test]
    fn clients_must_offer_utf8_to_initialize() {
        fn params(encodings: serde_json::Value) -> InitializeParams {
            serde_json::from_value(serde_json::json!({
                "process_id": null,
                "root_uri": null,
                "capabilities": { "general": { "positionEncodings": encodings } },
            }))
            .unwrap()
        }
        assert!(require_utf8_position_encoding(&params(serde_json::json!(["utf-8"]))).is_ok());
        assert!(
            require_utf8_position_encoding(&params(serde_json::json!(["utf-16", "utf-8"]))).is_ok()
        );
        // A client that cannot speak utf-8 is refused: answering utf-8 to it
        // would corrupt every position.
        assert!(require_utf8_position_encoding(&params(serde_json::json!(["utf-16"]))).is_err());
        assert!(require_utf8_position_encoding(&params(serde_json::json!([]))).is_err());
        // Silence means the utf-16 spec default — equally unservable.
        assert!(require_utf8_position_encoding(&params(serde_json::json!(null))).is_err());
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
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::write(
            root.path().join("README.not"),
            "= Surface title\n== Nested title\n\n```not\n= Hidden example\n```\n\n#heading[Explicit title]\n#code(text=\"fn main() {}\", lang=\"rust\", block=true)",
        )
        .unwrap();
        fs::write(root.path().join("child.not"), "child").unwrap();
        let root_path = dunce::canonicalize(root.path()).unwrap();
        let readme_path = dunce::canonicalize(root.path().join("README.not")).unwrap();
        let session = LspSession::new(root_path, true).unwrap();
        let state = session.request_context();
        let never = AtomicBool::new(false);
        let uri = file_path_to_uri(&readme_path).unwrap();

        let document = document_symbols(
            &state,
            DocumentSymbolParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            },
            &never,
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
            &never,
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
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::write(root.path().join("README.not"), "#heading(level=missing())").unwrap();
        let root_path = dunce::canonicalize(root.path()).unwrap();
        let session = LspSession::new(root_path, true).unwrap();
        let state = session.request_context();
        let never = AtomicBool::new(false);
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
        assert!(completion(&state, params, &never).unwrap().is_none());
    }

    #[test]
    fn completion_uses_unsaved_workspace_sources() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::write(root.path().join("README.not"), "#<ch").unwrap();
        fs::write(root.path().join("child.not"), "child").unwrap();
        let root_path = dunce::canonicalize(root.path()).unwrap();
        let mut session = LspSession::new(root_path.clone(), true).unwrap();
        let path = dunce::canonicalize(root.path().join("README.not")).unwrap();
        session.documents.insert(
            path.clone(),
            OpenDocument {
                version: 1,
                source: Arc::from("#<ch"),
            },
        );
        session.rebuild_blocking().unwrap();
        let state = session.request_context();
        let uri = file_path_to_uri(&path).unwrap();
        let params = CompletionParams {
            text_document_position: lsp_types::TextDocumentPositionParams::new(
                lsp_types::TextDocumentIdentifier::new(uri),
                Position::new(0, 3),
            ),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: None,
        };

        let never = AtomicBool::new(false);
        let Some(CompletionResponse::Array(items)) = completion(&state, params, &never).unwrap()
        else {
            panic!("expected completion items");
        };
        assert!(items.iter().any(|item| item.label == "child"));
    }

    #[test]
    fn keeps_marked_vaults_independent_within_one_worktree() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
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
        let mut session = LspSession::new(root_path, true).unwrap();
        let docs_readme = dunce::canonicalize(root.path().join("docs/README.not")).unwrap();
        let notes_readme = dunce::canonicalize(root.path().join("notes/README.not")).unwrap();

        assert_eq!(session.vaults.len(), 2);
        let state = session.request_context();
        let docs = workspace_for_source(&state, &docs_readme).unwrap();
        assert!(docs.sources.keys().any(|path| path.ends_with("guide.not")));
        assert!(
            !docs
                .sources
                .keys()
                .any(|path| path.ends_with("private.not"))
        );
        let notes = workspace_for_source(&state, &notes_readme).unwrap();
        assert!(
            notes
                .sources
                .keys()
                .any(|path| path.ends_with("private.not"))
        );

        let draft = root.path().join("docs/draft.not");
        session.documents.insert(
            draft.clone(),
            OpenDocument {
                version: 1,
                source: Arc::from("draft"),
            },
        );
        session.rebuild_blocking().unwrap();
        let state = session.request_context();
        let docs = workspace_for_source(&state, &draft).unwrap();
        assert!(docs.sources.keys().any(|path| path.ends_with("draft.not")));
        let notes = workspace_for_source(&state, &notes_readme).unwrap();
        assert!(!notes.sources.keys().any(|path| path.ends_with("draft.not")));
    }

    #[test]
    fn maps_service_severity_labels_to_lsp_severities() {
        let record = |severity: &str| DiagnosticRecord {
            path: None,
            source: None,
            range: None,
            code: "test".into(),
            severity: severity.into(),
            message: "m".into(),
        };
        assert_eq!(
            lsp_diagnostic(None, record("error")).severity,
            Some(DiagnosticSeverity::ERROR)
        );
        assert_eq!(
            lsp_diagnostic(None, record("warning")).severity,
            Some(DiagnosticSeverity::WARNING)
        );
        assert_eq!(
            lsp_diagnostic(None, record("info")).severity,
            Some(DiagnosticSeverity::INFORMATION)
        );
        assert_eq!(
            lsp_diagnostic(None, record("mystery")).severity,
            Some(DiagnosticSeverity::ERROR)
        );
    }

    #[test]
    fn distinguishes_error_code_classes() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::write(root.path().join("README.not"), "disk").unwrap();
        let root_path = dunce::canonicalize(root.path()).unwrap();
        let session = LspSession::new(root_path.clone(), true).unwrap();
        let state = session.request_context();
        let never = AtomicBool::new(false);

        // Malformed parameters are a client mistake (`InvalidParams`).
        let bad_params = handle_request(
            &state,
            Request::new(
                RequestId::from(1),
                HoverRequest::METHOD.into(),
                serde_json::json!({
                    "textDocument": {"uri": "file:///does-not-matter.not"},
                    "position": {"line": "not-a-number", "character": 0}
                }),
            ),
            &never,
        );
        assert_eq!(
            bad_params.response_result.unwrap_err().code,
            ErrorCode::InvalidParams as i32
        );

        // Unsupported methods are `MethodNotFound`, not a param failure.
        let unknown = handle_request(
            &state,
            Request::new(
                RequestId::from(2),
                "notist/unknown".into(),
                serde_json::json!({}),
            ),
            &never,
        );
        assert_eq!(
            unknown.response_result.unwrap_err().code,
            ErrorCode::MethodNotFound as i32
        );

        // A decodable request outside any known source degrades to a
        // graceful `null` result instead of an error.
        let unreachable_uri = file_path_to_uri(&root_path.join("missing.not")).unwrap();
        let graceful = handle_request(
            &state,
            Request::new(
                RequestId::from(3),
                HoverRequest::METHOD.into(),
                serde_json::json!({
                    "textDocument": {"uri": unreachable_uri.as_str()},
                    "position": {"line": 0, "character": 0}
                }),
            ),
            &never,
        );
        assert_eq!(graceful.response_result.unwrap(), serde_json::Value::Null);
    }

    #[test]
    fn applies_changes_in_arrival_order_and_records_versions() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::write(root.path().join("README.not"), "disk").unwrap();
        let root_path = dunce::canonicalize(root.path()).unwrap();
        let mut session = LspSession::new(root_path, true).unwrap();
        let path = dunce::canonicalize(root.path().join("README.not")).unwrap();
        let uri = file_path_to_uri(&path).unwrap();
        session
            .open(DidOpenTextDocumentParams {
                text_document: TextDocumentItem::new(uri, "notist".into(), 1, "first".into()),
            })
            .unwrap();
        let change_at = |version: i32, text: &str| DidChangeTextDocumentParams {
            text_document: lsp_types::VersionedTextDocumentIdentifier {
                uri: file_path_to_uri(&path).unwrap(),
                version,
            },
            content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: text.into(),
            }],
        };

        assert!(session.change(change_at(3, "third")).unwrap());
        // Versions gate nothing: the ordered stdio stream is the only
        // ordering authority (matching rust-analyzer and tinymist), so an
        // out-of-order version still applies and the latest version wins.
        assert!(session.change(change_at(2, "second")).unwrap());
        // A resend with identical content changes nothing but still records
        // the version (some clients resend on save).
        assert!(!session.change(change_at(2, "second")).unwrap());
        assert_eq!(
            session
                .documents
                .get(&path)
                .map(|document| document.version),
            Some(2)
        );
        assert_eq!(
            session
                .documents
                .get(&path)
                .map(|document| document.source.to_string()),
            Some("second".into())
        );
    }

    #[test]
    fn drops_changes_for_unopened_documents() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::write(root.path().join("README.not"), "disk").unwrap();
        let root_path = dunce::canonicalize(root.path()).unwrap();
        let mut session = LspSession::new(root_path, true).unwrap();
        let path = dunce::canonicalize(root.path().join("README.not")).unwrap();
        let change = DidChangeTextDocumentParams {
            text_document: lsp_types::VersionedTextDocumentIdentifier {
                uri: file_path_to_uri(&path).unwrap(),
                version: 1,
            },
            content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "ghost".into(),
            }],
        };

        // No didOpen preceded this change: applying it would be a guess, so
        // the notification is dropped instead of creating an overlay.
        assert!(!session.change(change).unwrap());
        assert!(session.documents.get(&path).is_none());
    }

    #[test]
    fn applies_incremental_and_batched_changes() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::write(root.path().join("README.not"), "disk").unwrap();
        let root_path = dunce::canonicalize(root.path()).unwrap();
        let mut session = LspSession::new(root_path, true).unwrap();
        let path = dunce::canonicalize(root.path().join("README.not")).unwrap();
        let uri = file_path_to_uri(&path).unwrap();
        let ranged = |line: u32, start: u32, end: u32, text: &str| {
            lsp_types::TextDocumentContentChangeEvent {
                range: Some(lsp_types::Range {
                    start: Position::new(line, start),
                    end: Position::new(line, end),
                }),
                range_length: None,
                text: text.into(),
            }
        };
        let whole = |text: &str| lsp_types::TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: text.into(),
        };
        let change = |content_changes: Vec<lsp_types::TextDocumentContentChangeEvent>| {
            DidChangeTextDocumentParams {
                text_document: lsp_types::VersionedTextDocumentIdentifier {
                    uri: file_path_to_uri(&path).unwrap(),
                    version: 1,
                },
                content_changes,
            }
        };

        // A ranged CJK edit applies in place of the whole document.
        session
            .open(DidOpenTextDocumentParams {
                text_document: TextDocumentItem::new(
                    uri.clone(),
                    "notist".into(),
                    1,
                    "alpha beta\ngamma\n".into(),
                ),
            })
            .unwrap();
        assert!(
            session
                .change(change(vec![ranged(0, 6, 10, "德贝塔")]))
                .unwrap()
        );
        assert_eq!(
            session.documents[&path].source.to_string(),
            "alpha 德贝塔\ngamma\n"
        );

        // A batch applies its changes in order; the second change crosses the
        // stale line-index prefix and forces a rebuild.
        assert!(
            session
                .change(change(vec![ranged(0, 0, 5, "ALPHA"), ranged(1, 0, 5, "Γ")]))
                .unwrap()
        );
        assert_eq!(
            session.documents[&path].source.to_string(),
            "ALPHA 德贝塔\nΓ\n"
        );

        // A whole-document change mid-batch becomes the base; only later
        // partial changes apply to it.
        assert!(
            session
                .change(change(vec![
                    ranged(0, 0, 1, "X"),
                    whole("fresh\n"),
                    ranged(0, 0, 5, "FRESH"),
                ]))
                .unwrap()
        );
        assert_eq!(session.documents[&path].source.to_string(), "FRESH\n");
    }

    #[test]
    fn preserves_multi_level_missing_parents_in_new_paths() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::create_dir(root.path().join("nested")).unwrap();
        let uri = file_path_to_uri(&root.path().join("nested/a/b/new.not")).unwrap();
        let normalized = normalize_uri_path(&uri).unwrap();
        assert_eq!(
            normalized,
            dunce::canonicalize(root.path().join("nested"))
                .unwrap()
                .join("a/b/new.not")
        );
    }

    #[test]
    fn applies_clamped_descending_and_crlf_batches() {
        let clamp = |text: &str, changes: &[TextDocumentContentChangeEvent]| {
            apply_document_changes(text, changes)
        };
        let ranged =
            |start: (u32, u32), end: (u32, u32), text: &str| TextDocumentContentChangeEvent {
                range: Some(lsp_types::Range {
                    start: Position::new(start.0, start.1),
                    end: Position::new(end.0, end.1),
                }),
                range_length: None,
                text: text.into(),
            };

        // A descending batch (the shape VS Code sends) reuses the line index
        // for the unchanged prefix: line 0 is untouched by the line-1 edit.
        let applied = clamp(
            "one two\nthree four\n",
            &[ranged((1, 6), (1, 10), "IV"), ranged((0, 0), (0, 3), "1")],
        );
        assert_eq!(applied, "1 two\nthree IV\n");

        // Ranges past the document clamp: an end beyond the source ends the
        // edit at the document end.
        let applied = clamp("alpha\nbeta\n", &[ranged((0, 5), (9, 40), "γ")]);
        assert_eq!(applied, "alphaγ");

        // CRLF content with UTF-8 byte columns: the column space excludes the
        // \r\n terminator, and `😀中` spans byte columns 2..9.
        let applied = clamp("ab😀中\r\nnext\r\n", &[ranged((0, 2), (0, 9), "😀!")]);
        assert_eq!(applied, "ab😀!\r\nnext\r\n");

        // A change whose clamped start exceeds its clamped end is skipped
        // instead of panicking or corrupting the buffer.
        let applied = clamp(
            "keep\n",
            &[ranged((0, 3), (0, 1), "GONE"), ranged((0, 4), (0, 4), "!")],
        );
        assert_eq!(applied, "keep!\n");
    }

    #[test]
    fn reports_unroutable_did_change_via_log_message() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::write(root.path().join("README.not"), "disk").unwrap();
        let root_path = dunce::canonicalize(root.path()).unwrap();
        let mut session = LspSession::new(root_path, true).unwrap();
        let path = dunce::canonicalize(root.path().join("README.not")).unwrap();
        let uri = file_path_to_uri(&path).unwrap();
        let (server, client) = Connection::memory();
        let change = |document_uri: &str, range: Option<lsp_types::Range>, text: &str| {
            Notification::new(
                DidChangeTextDocument::METHOD.into(),
                DidChangeTextDocumentParams {
                    text_document: lsp_types::VersionedTextDocumentIdentifier {
                        uri: document_uri.parse().expect("test uri parses"),
                        version: 2,
                    },
                    content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
                        range,
                        range_length: None,
                        text: text.into(),
                    }],
                },
            )
        };

        session
            .open(DidOpenTextDocumentParams {
                text_document: TextDocumentItem::new(
                    uri.clone(),
                    "notist".into(),
                    1,
                    "first".into(),
                ),
            })
            .unwrap();

        // An accepted incremental change stays silent.
        handle_notification(
            &server,
            &mut session,
            change(
                uri.as_str(),
                Some(lsp_types::Range {
                    start: Position::new(0, 0),
                    end: Position::new(0, 5),
                }),
                "second",
            ),
        );
        assert!(
            client
                .receiver
                .recv_timeout(Duration::from_millis(200))
                .is_err()
        );
        assert_eq!(
            session.documents[&path].source.to_string(),
            "second",
            "the ranged change applied"
        );

        // A URI that cannot route to a vault path must reach the client as a
        // log message; stderr alone leaves clients guessing why nothing
        // happens.
        handle_notification(
            &server,
            &mut session,
            change("untitled:Untitled-1", None, "orphan"),
        );
        let Message::Notification(notification) = client
            .receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
        else {
            panic!("expected log message notification");
        };
        assert_eq!(notification.method, LogMessage::METHOD);
        let params: LogMessageParams = serde_json::from_value(notification.params).unwrap();
        assert_eq!(params.typ, MessageType::WARNING);
        assert!(
            params.message.contains("rejected textDocument/didChange"),
            "{params:?}"
        );
    }

    #[test]
    fn apply_document_changes_matches_reference_on_random_edits() {
        // xorshift64*; the fixed seed keeps any failure reproducible.
        struct Rng(u64);
        impl Rng {
            fn next(&mut self) -> u64 {
                let mut x = self.0;
                x ^= x >> 12;
                x ^= x << 25;
                x ^= x >> 27;
                self.0 = x;
                x.wrapping_mul(0x2545_F491_4F6C_DD1D)
            }
            fn below(&mut self, bound: u64) -> u64 {
                (self.next() >> 11) % bound.max(1)
            }
        }
        // LineIndex splits lines on \n (so \r\n is one terminator); bare \r
        // is intentionally absent from the alphabet because it is not an LSP
        // line break this server models.
        const ALPHABET: [&str; 6] = ["a", "中", "😀", "\n", "\r\n", " "];
        fn insert_string(rng: &mut Rng) -> String {
            (0..rng.below(7))
                .map(|_| ALPHABET[rng.below(ALPHABET.len() as u64) as usize])
                .collect::<String>()
        }
        // Independent reference semantics for UTF-8 columns: manual line
        // splitting, line/content-end clamping, and a floor to the nearest
        // character boundary for columns that land inside one.
        fn naive_offset(text: &str, line: u32, character: u32) -> usize {
            let bytes = text.as_bytes();
            let mut bounds: Vec<(usize, usize)> = Vec::new();
            let mut start = 0;
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'\n' || bytes[i] == b'\r' {
                    bounds.push((start, i));
                    i += usize::from(bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n'));
                    i += 1;
                    start = i;
                } else {
                    i += 1;
                }
            }
            bounds.push((start, text.len()));
            let Some(&(line_start, content_end)) = bounds.get(line as usize) else {
                return text.len();
            };
            let mut target = line_start
                .saturating_add(character as usize)
                .min(content_end);
            while target > line_start && !text.is_char_boundary(target) {
                target -= 1;
            }
            target
        }

        let mut rng = Rng(0x5DEE_CE66);
        let mut expected = String::new();
        for step in 0..400 {
            let change = if rng.below(5) == 0 {
                TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: insert_string(&mut rng),
                }
            } else {
                // Deliberately out-of-bounds positions exercise clamping.
                let mut position = || Position::new(rng.below(8) as u32, rng.below(24) as u32);
                TextDocumentContentChangeEvent {
                    range: Some(lsp_types::Range {
                        start: position(),
                        end: position(),
                    }),
                    range_length: None,
                    text: insert_string(&mut rng),
                }
            };

            let mut reference = expected.clone();
            match &change.range {
                None => reference = change.text.clone(),
                Some(range) => {
                    let start = naive_offset(&reference, range.start.line, range.start.character);
                    let end = naive_offset(&reference, range.end.line, range.end.character);
                    if start <= end {
                        reference.replace_range(start..end, &change.text);
                    }
                }
            }

            expected = apply_document_changes(&expected, std::slice::from_ref(&change));
            assert_eq!(
                expected, reference,
                "diverged after step {step}: {change:?}"
            );
        }
    }

    #[test]
    fn protocol_loop_serves_overlay_diagnostics_completion_hover_and_definition() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        fs::write(root.path().join("README.not"), "disk").unwrap();
        fs::write(root.path().join("child.not"), "child").unwrap();
        let root_path = dunce::canonicalize(root.path()).unwrap();
        let readme_path = dunce::canonicalize(root.path().join("README.not")).unwrap();
        let child_path = dunce::canonicalize(root.path().join("child.not")).unwrap();
        let readme_uri = file_path_to_uri(&readme_path).unwrap();
        let source = "#<child> #heading[] #missing[]";
        let state = LspSession::new(root_path.clone(), true).unwrap();
        let (server, client) = Connection::memory();
        let server_thread = std::thread::spawn(move || {
            let runtime = Runtime::spawn(&server);
            main_loop(server, state, runtime).unwrap();
        });

        // The server must publish an initial diagnostics baseline before any
        // client activity; a silent start leaves clients with stale problems.
        loop {
            let Message::Notification(notification) = client
                .receiver
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
            else {
                panic!("expected initial diagnostics notification");
            };
            if notification.method == PublishDiagnostics::METHOD {
                break;
            }
        }

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
        for _ in 0..8 {
            let Message::Notification(notification) = client
                .receiver
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
            else {
                panic!("expected diagnostics notification");
            };
            if notification.method != PublishDiagnostics::METHOD {
                continue;
            }
            let params: PublishDiagnosticsParams =
                serde_json::from_value(notification.params).unwrap();
            if params.uri == readme_uri
                && params
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.message == "unknown function `missing`")
            {
                saw_overlay_diagnostic = true;
                break;
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
                        Position::new(0, 13),
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
                        Position::new(0, 10),
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
                        TextDocumentIdentifier::new(readme_uri.clone()),
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

        // Closing the document must clear its overlay diagnostics: the path
        // falls back to disk content and publishes an empty set.
        // Hover over a `let` binding resolves through the semantic index.
        let state = {
            let mut session = LspSession::new(root_path.clone(), true).unwrap();
            session.documents.insert(
                readme_path.clone(),
                OpenDocument {
                    version: 1,
                    source: Arc::from("#let greeting = \"hi\"\n#greeting"),
                },
            );
            session.rebuild_blocking().unwrap();
            session.request_context()
        };
        let never = AtomicBool::new(false);
        let hover_params = HoverParams {
            text_document_position_params: TextDocumentPositionParams::new(
                TextDocumentIdentifier::new(file_path_to_uri(&readme_path).unwrap()),
                Position::new(1, 2),
            ),
            work_done_progress_params: Default::default(),
        };
        let hovered = super::hover(&state, hover_params, &never).unwrap().unwrap();
        assert!(
            matches!(&hovered.contents,
                HoverContents::Markup(MarkupContent { value, .. }) if value.contains("`greeting:")
            ),
            "{:?}",
            hovered.contents
        );

        client
            .sender
            .send(Message::Notification(Notification::new(
                DidCloseTextDocument::METHOD.into(),
                DidCloseTextDocumentParams {
                    text_document: TextDocumentIdentifier::new(readme_uri.clone()),
                },
            )))
            .unwrap();
        let mut cleared_diagnostics = false;
        for _ in 0..8 {
            let Message::Notification(notification) = client
                .receiver
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
            else {
                panic!("expected diagnostics notification after close");
            };
            if notification.method != PublishDiagnostics::METHOD {
                continue;
            }
            let params: PublishDiagnosticsParams =
                serde_json::from_value(notification.params).unwrap();
            if params.uri == readme_uri {
                assert!(params.diagnostics.is_empty());
                cleared_diagnostics = true;
                break;
            }
        }
        assert!(cleared_diagnostics);

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
