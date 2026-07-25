use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use notist_service::protocol::{ClientKind, Handshake, ProtocolVersion};
use notist_service::transport::DaemonClient;
use notist_service::{CoreReply, CoreRequest, NotistService};

use crate::output::OutputFormat;

pub(crate) enum ClientBackend {
    Embedded(Arc<NotistService>),
    Daemon {
        runtime: tokio::runtime::Runtime,
        client: DaemonClient,
    },
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
        let runtime = tokio::runtime::Runtime::new()?;
        let handshake = handshake(kind, root.clone())?;
        let client = match runtime.block_on(DaemonClient::connect(&root, handshake.clone())) {
            Ok(client) => client,
            Err(error) if daemon_is_unavailable(&error) => {
                spawn_daemon(&root)?;
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    match runtime.block_on(DaemonClient::connect(&root, handshake.clone())) {
                        Ok(client) => break client,
                        Err(error)
                            if daemon_is_unavailable(&error) && Instant::now() < deadline =>
                        {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
            Err(error) => return Err(error),
        };
        Ok(Self {
            backend: ClientBackend::Daemon { runtime, client },
        })
    }

    pub fn request(&mut self, request: CoreRequest) -> io::Result<CoreReply> {
        match &mut self.backend {
            ClientBackend::Embedded(service) => service.execute(request),
            ClientBackend::Daemon { runtime, client } => runtime.block_on(client.request(request)),
        }
    }
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
    format: OutputFormat,
) -> Result<std::process::ExitCode, Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Runtime::new()?;
    let service = Arc::new(NotistService::for_root(&root)?);
    let vault_generation = crate::official_docs::generation_for_root(&root)?;
    if !background_child {
        if format.is_json() {
            crate::output::emit_event(
                "daemon",
                "started",
                serde_json::json!({
                    "root": root,
                    "instance_id": service.instance_id().0,
                    "vault_generation": vault_generation,
                }),
            )?;
        } else {
            eprintln!("notist daemon {}", service.instance_id().0);
        }
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
