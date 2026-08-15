use std::convert::Infallible;
use std::error::Error;
use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{Response, StatusCode, header};
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use clap::ColorChoice;
use notist_service::protocol::ClientKind;
use notist_service::{CoreRequest, CoreResponse, ProtocolViewKind, ServiceViewId};
use percent_encoding::percent_decode_str;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{StreamExt, once};

use crate::build::{SiteOptions, merge_diagnostics, render_workspace, write_rendered_site};
use crate::output::OutputFormat;
use crate::service::LocalNotistClient;

/// One event on the live-reload channel: a published site revision, or the
/// shutdown signal that ends every live-reload stream.
#[derive(Clone, Debug)]
enum PreviewEvent {
    Revision(u64),
    Shutdown,
}

pub fn run(
    root: PathBuf,
    host: IpAddr,
    port: u16,
    open: bool,
    color: ColorChoice,
    no_daemon: bool,
    format: OutputFormat,
) -> Result<ExitCode, Box<dyn Error>> {
    let root = dunce::canonicalize(root)?;
    let temporary = tempfile::tempdir()?;
    let site = PublishedSite::new(temporary.path().join("generations"))?;
    let revision = Arc::new(AtomicU64::new(1));
    let (updates, _) = broadcast::channel(16);

    let mut client = LocalNotistClient::connect(no_daemon, ClientKind::Preview, root.clone())?;
    let opened = client.request(CoreRequest::OpenView {
        root: root.clone(),
        kind: ProtocolViewKind::Disk,
    })?;
    let CoreResponse::Opened { view_id, .. } = opened.response else {
        return Err("service returned an unexpected open-view response".into());
    };
    let initial_snapshot_revision = opened.snapshot.revision;
    let diagnostics = rebuild_preview_site(&mut client, view_id, &site, color)?;
    print_rebuild_status(format, 1, &diagnostics)?;

    let rebuild_site = site.clone();
    let rebuild_revision = revision.clone();
    let rebuild_updates = updates.clone();
    let rebuild_stop = Arc::new(AtomicBool::new(false));
    let thread_stop = rebuild_stop.clone();
    let rebuild_thread = std::thread::Builder::new()
        .name("notist-preview-rebuild".into())
        .spawn(move || {
            let mut snapshot_revision = initial_snapshot_revision;
            while !thread_stop.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(250));
                // Retired site generations are deleted here, on the dedicated
                // worker thread, so page requests never pay the cost.
                rebuild_site.delete_retired();
                let summary = match client.request(CoreRequest::SnapshotSummary { view_id }) {
                    Ok(summary) => summary,
                    Err(error) => {
                        emit_preview_error(
                            format,
                            "snapshot_observation_failed",
                            &error.to_string(),
                        );
                        continue;
                    }
                };
                if summary.snapshot.revision == snapshot_revision {
                    continue;
                }
                snapshot_revision = summary.snapshot.revision;
                let rebuilt = rebuild_preview_site(&mut client, view_id, &rebuild_site, color);
                match rebuilt {
                    Ok(diagnostics) => {
                        let revision = rebuild_revision.fetch_add(1, Ordering::SeqCst) + 1;
                        let _ = rebuild_updates.send(PreviewEvent::Revision(revision));
                        if let Err(error) = print_rebuild_status(format, revision, &diagnostics) {
                            emit_preview_error(format, "output_failed", &error.to_string());
                        }
                    }
                    Err(error) => emit_preview_error(format, "rebuild_failed", &error.to_string()),
                }
            }
            rebuild_site.delete_retired();
        })?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let server_result = runtime.block_on(serve(
        site,
        host,
        port,
        open,
        revision,
        updates,
        rebuild_stop.clone(),
        format,
    ));

    rebuild_stop.store(true, Ordering::Release);
    if rebuild_thread.join().is_err() {
        emit_preview_error(
            format,
            "worker_stopped",
            "rebuild worker stopped unexpectedly",
        );
    }
    server_result?;
    Ok(ExitCode::SUCCESS)
}

fn rebuild_preview_site(
    client: &mut LocalNotistClient,
    view_id: ServiceViewId,
    site: &PublishedSite,
    _color: ColorChoice,
) -> Result<Vec<notist_service::DiagnosticRecord>, Box<dyn Error>> {
    let staging = site.next_generation_path();
    fs::create_dir_all(&staging)?;

    let rendered = render_workspace(client, view_id)?;
    write_rendered_site(&rendered, &staging, SiteOptions { live_reload: true })?;
    let mut diagnostics = rendered.analysis_diagnostics;
    merge_diagnostics(&mut diagnostics, rendered.evaluation_diagnostics);

    site.publish(staging);
    Ok(diagnostics)
}

fn print_rebuild_status(
    format: OutputFormat,
    revision: u64,
    diagnostics: &[notist_service::DiagnosticRecord],
) -> io::Result<()> {
    if format.is_json() {
        return crate::output::emit_event(
            "preview",
            "rebuilt",
            serde_json::json!({"revision": revision, "diagnostics": diagnostics}),
        );
    }
    crate::emit_service_diagnostics(diagnostics);
    if diagnostics.is_empty() {
        println!("preview revision {revision} built");
    } else {
        println!(
            "preview revision {revision} built with {} diagnostics",
            diagnostics.len()
        );
    }
    Ok(())
}

fn emit_preview_error(format: OutputFormat, event: &str, message: &str) {
    if format.is_json() {
        let _ =
            crate::output::emit_event("preview", event, serde_json::json!({"message": message}));
    } else {
        eprintln!("notist preview: {message}");
    }
}

#[derive(Clone)]
struct PreviewState {
    revision: Arc<AtomicU64>,
    updates: broadcast::Sender<PreviewEvent>,
    site: PublishedSite,
}

#[derive(Clone)]
struct PublishedSite {
    root: Arc<PathBuf>,
    current: Arc<RwLock<Arc<SiteGeneration>>>,
    next: Arc<AtomicU64>,
    /// Retired generation directories awaiting deletion. Deletion is deferred
    /// off the request path: `remove_dir_all` over a whole site generation is
    /// slow and must never block a tokio worker serving a page request.
    retired: Arc<Mutex<Vec<PathBuf>>>,
}

struct SiteGeneration {
    path: PathBuf,
    retired: Arc<Mutex<Vec<PathBuf>>>,
}

impl Drop for SiteGeneration {
    fn drop(&mut self) {
        // Enqueue, never delete inline: the last reference is usually released
        // by a `serve_static` request future, and blocking that worker stalls
        // the next request on the same keep-alive connection.
        let mut retired = self.retired.lock().unwrap_or_else(|error| error.into_inner());
        retired.push(self.path.clone());
    }
}

impl PublishedSite {
    fn new(root: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&root)?;
        let initial = root.join("generation-0");
        fs::create_dir_all(&initial)?;
        let retired = Arc::new(Mutex::new(Vec::new()));
        Ok(Self {
            root: Arc::new(root),
            current: Arc::new(RwLock::new(Arc::new(SiteGeneration {
                path: initial,
                retired: retired.clone(),
            }))),
            next: Arc::new(AtomicU64::new(1)),
            retired,
        })
    }

    fn next_generation_path(&self) -> PathBuf {
        let generation = self.next.fetch_add(1, Ordering::Relaxed);
        self.root.join(format!("generation-{generation}"))
    }

    fn publish(&self, path: PathBuf) {
        *self.current.write().unwrap() = Arc::new(SiteGeneration {
            path,
            retired: self.retired.clone(),
        });
    }

    fn capture(&self) -> Arc<SiteGeneration> {
        self.current.read().unwrap().clone()
    }

    /// Deletes every retired generation directory. Runs on the rebuild worker
    /// thread (or at shutdown), never on a request-serving thread.
    fn delete_retired(&self) {
        let retired = {
            let mut queue = self.retired.lock().unwrap_or_else(|error| error.into_inner());
            std::mem::take(&mut *queue)
        };
        for path in retired {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

async fn serve(
    site: PublishedSite,
    host: IpAddr,
    port: u16,
    open: bool,
    revision: Arc<AtomicU64>,
    updates: broadcast::Sender<PreviewEvent>,
    rebuild_stop: Arc<AtomicBool>,
    format: OutputFormat,
) -> Result<(), Box<dyn Error>> {
    if !host.is_loopback() {
        if format.is_json() {
            crate::output::emit_event(
                "preview",
                "warning",
                serde_json::json!({
                    "code": "non_loopback",
                    "message": format!("serving document content on non-loopback address {host}"),
                }),
            )?;
        } else {
            eprintln!(
                "notist preview: warning: serving document content on non-loopback address {host}"
            );
        }
    }

    // Try the requested port first; if it is taken, fall back to letting the
    // operating system choose an available ephemeral port (port 0).
    let listener = match tokio::net::TcpListener::bind(SocketAddr::new(host, port)).await {
        Ok(listener) => listener,
        Err(error)
            if port != 0
                && matches!(
                    error.kind(),
                    io::ErrorKind::AddrInUse | io::ErrorKind::AddrNotAvailable
                ) =>
        {
            if format.is_json() {
                crate::output::emit_event(
                    "preview",
                    "warning",
                    serde_json::json!({
                        "code": "port_in_use",
                        "message": format!(
                            "port {port} is unavailable, falling back to an ephemeral port"
                        ),
                    }),
                )?;
            } else {
                eprintln!(
                    "notist preview: warning: port {port} is unavailable, falling back to an ephemeral port"
                );
            }
            tokio::net::TcpListener::bind(SocketAddr::new(host, 0)).await?
        }
        Err(error) => return Err(error.into()),
    };
    let address = listener.local_addr()?;
    let state = PreviewState {
        revision,
        updates: updates.clone(),
        site,
    };
    let app = Router::new()
        .route("/_notist/events", get(events_response))
        .fallback(serve_static)
        .with_state(state);
    let browser_address = if address.ip().is_unspecified() {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), address.port())
    } else {
        address
    };
    let url = format!("http://{browser_address}/");
    if format.is_json() {
        crate::output::emit_event(
            "preview",
            "listening",
            serde_json::json!({"url": url, "address": address}),
        )?;
    } else {
        println!("preview server listening on {url}");
    }

    if open && let Err(error) = open::that_detached(&url) {
        emit_preview_error(format, "browser_open_failed", &error.to_string());
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(format, updates, rebuild_stop))
        .await?;
    Ok(())
}

async fn serve_static(State(state): State<PreviewState>, request: Request) -> impl IntoResponse {
    let Some(relative) = static_request_path(request.uri().path()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let generation = state.site.capture();
    let path = generation.path.join(relative);
    match tokio::fs::read(&path).await {
        Ok(contents) => {
            let content_type = match path.extension().and_then(|extension| extension.to_str()) {
                Some("css") => "text/css; charset=utf-8",
                Some("js") => "text/javascript; charset=utf-8",
                _ => "text/html; charset=utf-8",
            };
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, content_type)
                .body(Body::from(contents))
                .unwrap()
                .into_response()
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            StatusCode::NOT_FOUND.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn static_request_path(path: &str) -> Option<PathBuf> {
    let decoded = percent_decode_str(path).decode_utf8().ok()?;
    let mut relative = PathBuf::new();
    for segment in decoded.trim_start_matches('/').split('/') {
        if segment.is_empty() {
            continue;
        }
        if segment == "." || segment == ".." || segment.contains(['\\', '\0']) {
            return None;
        }
        relative.push(segment);
    }
    if decoded.ends_with('/') || relative.as_os_str().is_empty() {
        relative.push("index.html");
    }
    Some(relative)
}

async fn events_response(
    State(state): State<PreviewState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    Sse::new(revision_stream(state.revision.clone(), state.updates.clone()))
        .keep_alive(KeepAlive::default())
}

/// The live-reload revision stream: the current revision, then revisions as
/// they are published, ending when the `Shutdown` event arrives so Ctrl+C can
/// complete the graceful drain.
fn revision_stream(
    revision: Arc<AtomicU64>,
    updates: broadcast::Sender<PreviewEvent>,
) -> impl tokio_stream::Stream<Item = Result<Event, Infallible>> + Send {
    let current = revision.load(Ordering::SeqCst);
    let initial = once(Ok(Event::default().data(current.to_string())));
    let revisions = BroadcastStream::new(updates.subscribe())
        .filter_map(|result| result.ok())
        .take_while(|event| matches!(event, PreviewEvent::Revision(_)))
        .map(|event| match event {
            PreviewEvent::Revision(revision) => Ok(Event::default().data(revision.to_string())),
            PreviewEvent::Shutdown => unreachable!("take_while ends the stream on Shutdown"),
        });
    initial.chain(revisions)
}

async fn shutdown_signal(
    format: OutputFormat,
    updates: broadcast::Sender<PreviewEvent>,
    rebuild_stop: Arc<AtomicBool>,
) {
    if let Err(error) = tokio::signal::ctrl_c().await {
        emit_preview_error(format, "shutdown_signal_failed", &error.to_string());
    }
    // Stop the rebuild worker immediately (no more error spam against a
    // possibly dead service) and end every live-reload stream so the server
    // can drain its connections and exit.
    rebuild_stop.store(true, Ordering::Release);
    let _ = updates.send(PreviewEvent::Shutdown);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_rebuild_swaps_a_complete_live_reload_site() {
        let root = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        let output = tempfile::TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
        let site = PublishedSite::new(output.path().join("generations")).unwrap();
        fs::write(root.path().join("README.not"), "#heading[First]").unwrap();
        let mut client =
            LocalNotistClient::connect(true, ClientKind::Preview, root.path().to_path_buf())
                .unwrap();
        let opened = client
            .request(CoreRequest::OpenView {
                root: root.path().to_path_buf(),
                kind: ProtocolViewKind::Disk,
            })
            .unwrap();
        let CoreResponse::Opened { view_id, .. } = opened.response else {
            panic!("expected open view")
        };

        let diagnostics =
            rebuild_preview_site(&mut client, view_id, &site, ColorChoice::Never).unwrap();

        assert!(diagnostics.is_empty());
        let first_generation = site.capture();
        assert!(first_generation.path.join("_notist/reload.js").is_file());
        let first = fs::read_to_string(first_generation.path.join("index.html")).unwrap();
        assert!(!first.contains("#heading"));
        assert!(first.contains(">First</span>"));
        assert!(first.contains("_notist/reload.js"));

        fs::write(root.path().join("README.not"), "#heading[Second]").unwrap();
        client
            .request(CoreRequest::ReloadDiskView { view_id })
            .unwrap();
        rebuild_preview_site(&mut client, view_id, &site, ColorChoice::Never).unwrap();
        let second_generation = site.capture();
        let second = fs::read_to_string(second_generation.path.join("index.html")).unwrap();
        assert!(second.contains(">Second</span>"));
        assert!(!second.contains(">First</span>"));
        assert!(first_generation.path.is_dir());
        let first_path = first_generation.path.clone();
        drop(first_generation);
        // Deletion is deferred: the drop never blocks the caller, so the
        // retired directory survives until the rebuild worker drains it.
        assert!(first_path.exists());
        site.delete_retired();
        assert!(!first_path.exists());
    }

    #[test]
    fn static_paths_reject_traversal_and_map_clean_urls() {
        assert_eq!(static_request_path("/"), Some(PathBuf::from("index.html")));
        assert_eq!(
            static_request_path("/notes/today/"),
            Some(PathBuf::from("notes/today/index.html"))
        );
        assert!(static_request_path("/%2e%2e/secret").is_none());
        assert!(static_request_path("/notes%5csecret").is_none());
    }

    #[tokio::test]
    async fn revision_stream_ends_on_shutdown_event() {
        let (updates, _) = broadcast::channel(16);
        let revision = Arc::new(AtomicU64::new(1));
        let mut stream = revision_stream(revision.clone(), updates.clone());

        assert!(stream.next().await.is_some());

        updates.send(PreviewEvent::Revision(3)).unwrap();
        assert!(stream.next().await.is_some());

        // The Shutdown event ends the stream: this is what lets the graceful
        // drain finish while a browser holds an EventSource open.
        updates.send(PreviewEvent::Shutdown).unwrap();
        assert!(stream.next().await.is_none());
    }
}
