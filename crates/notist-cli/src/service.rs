use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use notist_service::protocol::{ClientKind, Handshake, ProtocolVersion};
use notist_service::transport::{DaemonClient, ShutdownReply};
use notist_service::{CoreReply, CoreRequest, NotistService};

pub(crate) enum ClientBackend {
    Embedded(Arc<NotistService>),
    Daemon {
        runtime: Arc<tokio::runtime::Runtime>,
        client: Arc<DaemonClient>,
    },
}

/// A shareable request entry point derived from one connection. Embedded
/// services execute on `&self`, so same-vault queries run concurrently; the
/// daemon client is multiplexed (id-routed replies), so one connection serves
/// concurrent in-flight requests too. Requests may interleave freely on the
/// wire — callers that need ordering sequence their own requests, which the
/// LSP builder already does.
#[derive(Clone)]
pub(crate) enum RequestHandle {
    Embedded(Arc<NotistService>),
    Daemon {
        runtime: Arc<tokio::runtime::Runtime>,
        client: Arc<DaemonClient>,
    },
}

impl RequestHandle {
    pub fn request(&self, request: CoreRequest) -> io::Result<CoreReply> {
        match self {
            Self::Embedded(service) => service.execute(request),
            Self::Daemon { runtime, client } => {
                runtime.block_on(async { client.send(request)?.wait().await })
            }
        }
    }

    /// Issues a request that observes `cancelled`: embedded execution checks
    /// the flag at entry and inside long operations; daemon requests poll the
    /// flag and deliver a protocol `Cancel` so the daemon aborts the
    /// computation and replies "request cancelled".
    pub fn cancellable(
        &self,
        request: CoreRequest,
        cancelled: &AtomicBool,
    ) -> io::Result<CoreReply> {
        match self {
            Self::Embedded(service) => service.execute_cancellable(request, cancelled),
            Self::Daemon { runtime, client } => runtime.block_on(async {
                let mut inflight = client.send(request)?;
                tokio::select! {
                    reply = inflight.wait() => reply,
                    _ = cancelled_poll(cancelled) => {
                        client.cancel(inflight.id()).await?;
                        // The daemon still answers (with the cancelled error);
                        // waiting for it keeps the pending table clean.
                        inflight.wait().await
                    }
                }
            }),
        }
    }
}

/// Resolves when the cooperative flag is set. Polled every few milliseconds:
/// cancellation latency is bounded by the poll, which is fine for the
/// long operations that need it.
async fn cancelled_poll(cancelled: &AtomicBool) {
    while !cancelled.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

pub(crate) struct LocalNotistClient {
    backend: ClientBackend,
}

impl LocalNotistClient {
    pub fn connect(no_daemon: bool, kind: ClientKind, root: PathBuf) -> io::Result<Self> {
        let root = dunce::canonicalize(root)?;
        if no_daemon {
            return Ok(Self {
                backend: ClientBackend::Embedded(Arc::new(NotistService::for_root(&root)?)),
            });
        }
        let runtime = Arc::new(tokio::runtime::Runtime::new()?);
        let our_stamp = notist_service::transport::binary_stamp();
        let client = Arc::new(connect_daemon(&runtime, &root, kind, our_stamp)?);
        Ok(Self {
            backend: ClientBackend::Daemon { runtime, client },
        })
    }

    pub fn request(&self, request: CoreRequest) -> io::Result<CoreReply> {
        match &self.backend {
            ClientBackend::Embedded(service) => service.execute(request),
            ClientBackend::Daemon { runtime, client } => {
                runtime.block_on(async { client.send(request)?.wait().await })
            }
        }
    }

    /// Consumes the client into a shareable handle for concurrent use.
    pub fn into_request_handle(self) -> RequestHandle {
        match self.backend {
            ClientBackend::Embedded(service) => RequestHandle::Embedded(service),
            ClientBackend::Daemon { runtime, client } => RequestHandle::Daemon { runtime, client },
        }
    }
}

const DAEMON_CONNECT_DEADLINE: Duration = Duration::from_secs(5);

/// Connect to the vault daemon, starting it on demand. When the running daemon
/// was started from a different executable than this process, it is asked to
/// shut down (unless it has other active clients) and a replacement daemon is
/// started from the current executable, so local `cargo run` development never
/// silently queries code that predates the last build.
fn connect_daemon(
    runtime: &tokio::runtime::Runtime,
    root: &Path,
    kind: ClientKind,
    our_stamp: Option<u64>,
) -> io::Result<DaemonClient> {
    let mut restarts = 0;
    loop {
        let client = connect_once(runtime, root, kind)?;
        match recycle_stale_daemon(runtime, root, client, our_stamp, restarts >= 2)? {
            Recycle::Keep(client) => return Ok(client),
            Recycle::Restart => restarts += 1,
        }
    }
}

enum Recycle {
    /// Use the connected daemon as-is.
    Keep(DaemonClient),
    /// The stale daemon was shut down; connect to the replacement instead.
    Restart,
}

/// One connect attempt that also starts the daemon when it is unavailable and
/// waits briefly for it to come up.
fn connect_once(
    runtime: &tokio::runtime::Runtime,
    root: &Path,
    kind: ClientKind,
) -> io::Result<DaemonClient> {
    let hs = handshake(kind, root.to_path_buf())?;
    match runtime.block_on(DaemonClient::connect(root, hs)) {
        Ok(client) => Ok(client),
        Err(error) if daemon_is_unavailable(&error) => {
            spawn_daemon(root)?;
            let deadline = Instant::now() + DAEMON_CONNECT_DEADLINE;
            loop {
                std::thread::sleep(Duration::from_millis(50));
                let hs = handshake(kind, root.to_path_buf())?;
                match runtime.block_on(DaemonClient::connect(root, hs)) {
                    Ok(client) => return Ok(client),
                    Err(error) if daemon_is_unavailable(&error) && Instant::now() < deadline => {}
                    Err(error) => return Err(error),
                }
            }
        }
        Err(error) => Err(error),
    }
}

/// If the connected daemon serves a different binary than this process, ask it
/// to shut down (guarded by "no other active clients") and signal a restart.
fn recycle_stale_daemon(
    runtime: &tokio::runtime::Runtime,
    root: &Path,
    client: DaemonClient,
    our_stamp: Option<u64>,
    restart_exhausted: bool,
) -> io::Result<Recycle> {
    let stale = match (client.handshake.daemon_binary_stamp, our_stamp) {
        (Some(daemon_stamp), Some(our_stamp)) => daemon_stamp != our_stamp,
        _ => false,
    };
    if !stale {
        return Ok(Recycle::Keep(client));
    }
    if restart_exhausted {
        eprintln!(
            "notist: the daemon for {} serves a different binary and keeps reappearing; keeping this connection. Run `notist daemon stop <root>` to restart it.",
            root.display()
        );
        return Ok(Recycle::Keep(client));
    }
    match runtime.block_on(client.shutdown(false))? {
        ShutdownReply::Accepted { .. } => {
            drop(client);
            // Wait until the old daemon has released its endpoint; the caller's
            // next connect attempt then starts the replacement through the
            // normal spawn-on-unavailable path.
            wait_for_endpoint_free(runtime, root)?;
            Ok(Recycle::Restart)
        }
        ShutdownReply::Rejected { message } => {
            eprintln!(
                "notist: the daemon for {} serves a different binary and has active clients ({message}); keeping this connection. Run `notist daemon stop <root>` to restart it.",
                root.display()
            );
            Ok(Recycle::Keep(client))
        }
    }
}

/// Poll the endpoint until no daemon answers, meaning the old process has
/// released it. Each poll opens a throwaway connection that is dropped.
fn wait_for_endpoint_free(runtime: &tokio::runtime::Runtime, root: &Path) -> io::Result<()> {
    let deadline = Instant::now() + DAEMON_CONNECT_DEADLINE;
    loop {
        let hs = handshake(ClientKind::Cli, root.to_path_buf())?;
        match runtime.block_on(DaemonClient::connect(root, hs)) {
            Ok(_client) => {}
            Err(error) if daemon_is_unavailable(&error) => return Ok(()),
            Err(error) => return Err(error),
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "daemon did not exit after shutdown",
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Stop the daemon serving `root`. A missing daemon is not an error, so scripts
/// can run `notist daemon stop` before a rebuild without failing.
pub(crate) fn stop_daemon(root: PathBuf) -> io::Result<()> {
    let root = dunce::canonicalize(root)?;
    let runtime = tokio::runtime::Runtime::new()?;
    let hs = handshake(ClientKind::Cli, root.clone())?;
    let client = match runtime.block_on(DaemonClient::connect(&root, hs)) {
        Ok(client) => client,
        Err(error) if daemon_is_unavailable(&error) => {
            println!("no notist daemon is running for {}", root.display());
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    match runtime.block_on(client.shutdown(true))? {
        ShutdownReply::Accepted { pid } => {
            wait_for_endpoint_free(&runtime, &root)?;
            if let Some(pid) = pid {
                println!("stopped notist daemon (pid {pid})");
            } else {
                println!("stopped notist daemon");
            }
        }
        ShutdownReply::Rejected { message } => {
            return Err(io::Error::other(format!(
                "daemon refused to stop: {message}"
            )));
        }
    }
    Ok(())
}

fn handshake(kind: ClientKind, vault_root: PathBuf) -> io::Result<Handshake> {
    let vault_generation = crate::official_docs::generation_for_root(&vault_root)?;
    Ok(Handshake {
        protocol_version: ProtocolVersion::CURRENT,
        client_kind: kind,
        client_version: env!("CARGO_PKG_VERSION").into(),
        vault_root,
        vault_generation,
        requested_capabilities: vec![
            "completion".into(),
            "definition".into(),
            "diagnostics".into(),
            "hover".into(),
            "references".into(),
            "search".into(),
            "bounded_query".into(),
            "read_source".into(),
            "symbols".into(),
        ],
    })
}

fn daemon_is_unavailable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::BrokenPipe
    ) || matches!(error.raw_os_error(), Some(2 | 53 | 109 | 231 | 233))
}

fn spawn_daemon(root: &Path) -> io::Result<()> {
    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .arg("daemon")
        .arg("--vault")
        .arg(root)
        .arg("--background-child")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    command.spawn()?;
    Ok(())
}

pub(crate) fn run_daemon(
    root: PathBuf,
    background_child: bool,
) -> Result<std::process::ExitCode, Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Runtime::new()?;
    let service = Arc::new(NotistService::for_daemon_root(&root)?);
    let vault_generation = crate::official_docs::generation_for_root(&root)?;
    if !background_child {
        eprintln!("notist daemon {}", service.instance_id().0);
    }
    let idle_timeout = background_child.then_some(Duration::from_secs(5 * 60));
    runtime.block_on(notist_service::transport::serve(
        root,
        service,
        idle_timeout,
        vault_generation,
    ))?;
    Ok(std::process::ExitCode::SUCCESS)
}
