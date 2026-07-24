use std::convert::Infallible;
use std::error::Error;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use clap::ColorChoice;
use notify_debouncer_mini::notify::RecursiveMode;
use notify_debouncer_mini::{DebounceEventResult, new_debouncer};
use notist_analysis::VaultEngine;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{StreamExt, once};
use tower_http::services::ServeDir;

use crate::build::{SiteOptions, build_site, diagnostic_count, emit_diagnostics};

pub fn run(
    root: PathBuf,
    host: IpAddr,
    port: u16,
    no_open: bool,
    color: ColorChoice,
) -> Result<ExitCode, Box<dyn Error>> {
    let root = dunce::canonicalize(root)?;
    let temporary = tempfile::tempdir()?;
    let site = temporary.path().join("site");
    let revision = Arc::new(AtomicU64::new(1));
    let (updates, _) = broadcast::channel(16);

    let engine = VaultEngine::open(&root)?;
    let mut view = engine.disk_view()?;
    let diagnostics = rebuild_preview_site(view.current(), &site, color)?;
    print_rebuild_status(1, diagnostics);

    let (event_tx, event_rx) = mpsc::sync_channel(1);
    let mut debouncer = new_debouncer(
        Duration::from_millis(250),
        move |result: DebounceEventResult| match result {
            Ok(events) if !events.is_empty() => {
                let _ = event_tx.try_send(());
            }
            Ok(_) => {}
            Err(error) => eprintln!("notist preview: file watcher error: {error}"),
        },
    )?;
    debouncer.watcher().watch(&root, RecursiveMode::Recursive)?;

    let rebuild_site = site.clone();
    let rebuild_revision = revision.clone();
    let rebuild_updates = updates.clone();
    let rebuild_thread = std::thread::Builder::new()
        .name("notist-preview-rebuild".into())
        .spawn(move || {
            while event_rx.recv().is_ok() {
                while event_rx.try_recv().is_ok() {}
                match view
                    .reload()
                    .map_err(Into::into)
                    .and_then(|snapshot| rebuild_preview_site(&snapshot, &rebuild_site, color))
                {
                    Ok(diagnostics) => {
                        let revision = rebuild_revision.fetch_add(1, Ordering::SeqCst) + 1;
                        let _ = rebuild_updates.send(revision);
                        print_rebuild_status(revision, diagnostics);
                    }
                    Err(error) => eprintln!("notist preview: rebuild failed: {error}"),
                }
            }
        })?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let server_result = runtime.block_on(serve(site, host, port, no_open, revision, updates));

    drop(debouncer);
    if rebuild_thread.join().is_err() {
        eprintln!("notist preview: rebuild worker stopped unexpectedly");
    }
    server_result?;
    Ok(ExitCode::SUCCESS)
}

fn rebuild_preview_site(
    workspace: &notist_analysis::WorkspaceSnapshot,
    site: &Path,
    color: ColorChoice,
) -> Result<usize, Box<dyn Error>> {
    let staging = site.with_extension("next");
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(&staging)?;

    let result = build_site(workspace, &staging, SiteOptions { live_reload: true })?;
    let diagnostics = diagnostic_count(workspace, &result);

    if site.exists() {
        fs::remove_dir_all(site)?;
    }
    fs::rename(&staging, site)?;
    emit_diagnostics(workspace, &result, color)?;
    Ok(diagnostics)
}

fn print_rebuild_status(revision: u64, diagnostics: usize) {
    if diagnostics == 0 {
        println!("preview revision {revision} built");
    } else {
        println!("preview revision {revision} built with {diagnostics} diagnostics");
    }
}

#[derive(Clone)]
struct PreviewState {
    revision: Arc<AtomicU64>,
    updates: broadcast::Sender<u64>,
}

async fn serve(
    site: PathBuf,
    host: IpAddr,
    port: u16,
    no_open: bool,
    revision: Arc<AtomicU64>,
    updates: broadcast::Sender<u64>,
) -> Result<(), Box<dyn Error>> {
    if !host.is_loopback() {
        eprintln!(
            "notist preview: warning: serving document content on non-loopback address {host}"
        );
    }

    let listener = tokio::net::TcpListener::bind(SocketAddr::new(host, port)).await?;
    let address = listener.local_addr()?;
    let state = PreviewState { revision, updates };
    let app = Router::new()
        .route("/_notist/events", get(events_response))
        .fallback_service(ServeDir::new(site).append_index_html_on_directories(true))
        .with_state(state);
    let browser_address = if address.ip().is_unspecified() {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), address.port())
    } else {
        address
    };
    let url = format!("http://{browser_address}/");
    println!("preview server listening on {url}");

    if !no_open && let Err(error) = open::that_detached(&url) {
        eprintln!("notist preview: failed to open browser: {error}");
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn events_response(
    State(state): State<PreviewState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let current = state.revision.load(Ordering::SeqCst);
    let initial = once(Ok(Event::default().data(current.to_string())));
    let updates = BroadcastStream::new(state.updates.subscribe()).filter_map(|result| {
        result
            .ok()
            .map(|revision| Ok(Event::default().data(revision.to_string())))
    });
    Sse::new(initial.chain(updates)).keep_alive(KeepAlive::default())
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        eprintln!("notist preview: failed to listen for shutdown signal: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_rebuild_swaps_a_complete_live_reload_site() {
        let root = tempfile::TempDir::new().unwrap();
        let output = tempfile::TempDir::new().unwrap();
        let site = output.path().join("site");
        fs::write(root.path().join("README.not"), "#heading[First]").unwrap();
        let engine = VaultEngine::open(root.path()).unwrap();
        let mut view = engine.disk_view().unwrap();

        let diagnostics = rebuild_preview_site(view.current(), &site, ColorChoice::Never).unwrap();

        assert_eq!(diagnostics, 0);
        assert!(site.join("_notist/reload.js").is_file());
        let first = fs::read_to_string(site.join("index.html")).unwrap();
        assert!(!first.contains("#heading"));
        assert!(first.contains(">First</span>"));
        assert!(first.contains("_notist/reload.js"));

        fs::write(root.path().join("README.not"), "#heading[Second]").unwrap();
        let snapshot = view.reload().unwrap();
        rebuild_preview_site(&snapshot, &site, ColorChoice::Never).unwrap();
        let second = fs::read_to_string(site.join("index.html")).unwrap();
        assert!(second.contains(">Second</span>"));
        assert!(!second.contains(">First</span>"));
    }
}
