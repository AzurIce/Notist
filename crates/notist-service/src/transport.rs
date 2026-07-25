//! User-private, vault-scoped IPC transport for daemon and client processes.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::protocol::{Handshake, HandshakeAccepted, negotiate};
use crate::{CoreReply, CoreRequest, CoreResponse, NotistService, ServiceViewId};

const MAX_FRAME_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    Handshake { handshake: Handshake },
    Request { id: u64, request: CoreRequest },
    Cancel { id: u64 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage {
    HandshakeAccepted {
        accepted: HandshakeAccepted,
    },
    HandshakeRejected {
        message: String,
    },
    Response {
        id: u64,
        result: Result<CoreReply, String>,
    },
}

pub struct DaemonClient {
    stream: ClientStream,
    next_id: u64,
    pub handshake: HandshakeAccepted,
}

impl DaemonClient {
    pub async fn connect(root: &Path, handshake: Handshake) -> io::Result<Self> {
        let mut stream = connect_stream(root).await?;
        write_frame(
            &mut stream,
            &ClientMessage::Handshake {
                handshake: handshake.clone(),
            },
        )
        .await?;
        let response: ServerMessage = read_frame(&mut stream).await?;
        let accepted = match response {
            ServerMessage::HandshakeAccepted { accepted } => accepted,
            ServerMessage::HandshakeRejected { message } => {
                return Err(io::Error::new(io::ErrorKind::Unsupported, message));
            }
            ServerMessage::Response { .. } => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "daemon sent a response before handshake",
                ));
            }
        };
        Ok(Self {
            stream,
            next_id: 1,
            handshake: accepted,
        })
    }

    pub async fn request(&mut self, request: CoreRequest) -> io::Result<CoreReply> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("local request ID overflow"))?;
        write_frame(&mut self.stream, &ClientMessage::Request { id, request }).await?;
        match read_frame(&mut self.stream).await? {
            ServerMessage::Response {
                id: response_id,
                result,
            } if response_id == id => result.map_err(io::Error::other),
            ServerMessage::Response { .. } => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "daemon returned an out-of-order response",
            )),
            ServerMessage::HandshakeAccepted { .. } | ServerMessage::HandshakeRejected { .. } => {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "daemon repeated the handshake",
                ))
            }
        }
    }

    pub async fn cancel(&mut self, id: u64) -> io::Result<()> {
        write_frame(&mut self.stream, &ClientMessage::Cancel { id }).await
    }
}

pub async fn serve(
    root: PathBuf,
    service: Arc<NotistService>,
    idle_timeout: Option<Duration>,
) -> io::Result<()> {
    serve_platform(root, service, idle_timeout).await
}

async fn serve_connection<S>(
    mut stream: S,
    root: Arc<PathBuf>,
    service: Arc<NotistService>,
) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let handshake = match read_frame::<_, ClientMessage>(&mut stream).await? {
        ClientMessage::Handshake { handshake } => handshake,
        ClientMessage::Request { .. } | ClientMessage::Cancel { .. } => {
            write_frame(
                &mut stream,
                &ServerMessage::HandshakeRejected {
                    message: "handshake required before requests".into(),
                },
            )
            .await?;
            return Ok(());
        }
    };
    let mut accepted = match negotiate(&handshake) {
        Ok(accepted) => accepted,
        Err(message) => {
            write_frame(&mut stream, &ServerMessage::HandshakeRejected { message }).await?;
            return Ok(());
        }
    };
    if handshake.vault_root != *root {
        write_frame(
            &mut stream,
            &ServerMessage::HandshakeRejected {
                message: format!(
                    "daemon serves `{}`, not `{}`",
                    root.display(),
                    handshake.vault_root.display()
                ),
            },
        )
        .await?;
        return Ok(());
    }
    accepted.daemon_instance = service.instance_id().0.clone();
    write_frame(&mut stream, &ServerMessage::HandshakeAccepted { accepted }).await?;

    let (mut reader, mut writer) = tokio::io::split(stream);
    let (outgoing, mut outgoing_rx) = mpsc::channel::<ServerMessage>(32);
    let writer_task = tokio::spawn(async move {
        while let Some(message) = outgoing_rx.recv().await {
            write_frame(&mut writer, &message).await?;
        }
        Ok::<(), io::Error>(())
    });
    let cancellations: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let leases: Arc<Mutex<Vec<ServiceViewId>>> = Arc::new(Mutex::new(Vec::new()));
    let mut requests = JoinSet::new();

    loop {
        let message = match read_frame::<_, ClientMessage>(&mut reader).await {
            Ok(message) => message,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error),
        };
        match message {
            ClientMessage::Handshake { .. } => break,
            ClientMessage::Cancel { id } => {
                if let Some(cancelled) = cancellations.lock().unwrap().get(&id) {
                    cancelled.store(true, Ordering::Release);
                }
            }
            ClientMessage::Request { id, request } => {
                if let CoreRequest::OpenView {
                    root: requested_root,
                    ..
                } = &request
                {
                    let requested_root = dunce::canonicalize(requested_root)?;
                    if requested_root != *root {
                        let _ = outgoing
                            .send(ServerMessage::Response {
                                id,
                                result: Err("daemon only serves its configured vault".into()),
                            })
                            .await;
                        continue;
                    }
                }
                if let Some(view_id) = request.view_id()
                    && !leases.lock().unwrap().contains(&view_id)
                {
                    let _ = outgoing
                        .send(ServerMessage::Response {
                            id,
                            result: Err("view handle is not owned by this connection".into()),
                        })
                        .await;
                    continue;
                }
                let cancelled = Arc::new(AtomicBool::new(false));
                cancellations.lock().unwrap().insert(id, cancelled.clone());
                let outgoing = outgoing.clone();
                let cancellations = cancellations.clone();
                let leases = leases.clone();
                let service = service.clone();
                requests.spawn(async move {
                    let requested_close = matches!(request, CoreRequest::CloseView { .. });
                    let request_cancelled = cancelled.clone();
                    let result = tokio::task::spawn_blocking(move || {
                        service.execute_cancellable(request, &request_cancelled)
                    })
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result.map_err(|error| error.to_string()));
                    cancellations.lock().unwrap().remove(&id);
                    let result = if cancelled.load(Ordering::Acquire) {
                        Err("request cancelled".into())
                    } else {
                        result
                    };
                    if let Ok(CoreReply {
                        response: CoreResponse::Opened { view_id, .. },
                        ..
                    }) = &result
                    {
                        leases.lock().unwrap().push(*view_id);
                    } else if requested_close && let Ok(reply) = &result {
                        let closed = reply.snapshot.view_id;
                        leases.lock().unwrap().retain(|view| *view != closed);
                    }
                    let _ = outgoing.send(ServerMessage::Response { id, result }).await;
                });
            }
        }
    }

    while requests.join_next().await.is_some() {}
    for view in std::mem::take(&mut *leases.lock().unwrap()) {
        service.close_view(view);
    }
    drop(outgoing);
    writer_task.await.map_err(io::Error::other)??;
    Ok(())
}

async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(
    writer: &mut W,
    message: &T,
) -> io::Result<()> {
    let payload = serde_json::to_vec(message).map_err(io::Error::other)?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "local protocol frame exceeds size limit",
        ));
    }
    writer.write_u32_le(payload.len() as u32).await?;
    writer.write_all(&payload).await?;
    writer.flush().await
}

async fn read_frame<R: AsyncRead + Unpin, T: serde::de::DeserializeOwned>(
    reader: &mut R,
) -> io::Result<T> {
    let length = reader.read_u32_le().await? as usize;
    if length > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "local protocol frame exceeds size limit",
        ));
    }
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload).await?;
    serde_json::from_slice(&payload).map_err(io::Error::other)
}

fn endpoint_discriminator(root: &Path) -> String {
    let identity = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .unwrap_or_default();
    let mut hash = 0xcbf29ce484222325u64;
    for byte in identity.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    for byte in root.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(windows)]
type ClientStream = tokio::net::windows::named_pipe::NamedPipeClient;

#[cfg(windows)]
async fn connect_stream(root: &Path) -> io::Result<ClientStream> {
    use tokio::net::windows::named_pipe::ClientOptions;

    let endpoint = format!(r"\\.\pipe\notist-{}", endpoint_discriminator(root));
    let mut attempts = 0;
    loop {
        match ClientOptions::new().open(&endpoint) {
            Ok(client) => return Ok(client),
            Err(error) if attempts < 20 && error.raw_os_error() == Some(231) => {
                attempts += 1;
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(windows)]
async fn serve_platform(
    root: PathBuf,
    service: Arc<NotistService>,
    idle_timeout: Option<Duration>,
) -> io::Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let endpoint = format!(r"\\.\pipe\notist-{}", endpoint_discriminator(&root));
    let root = Arc::new(root);
    let active = Arc::new(AtomicUsize::new(0));
    let mut first = true;
    loop {
        let mut options = ServerOptions::new();
        options
            .first_pipe_instance(first)
            .reject_remote_clients(true);
        let server = options.create(&endpoint)?;
        first = false;
        if let Some(idle_timeout) = idle_timeout {
            match tokio::time::timeout(idle_timeout, server.connect()).await {
                Ok(result) => result?,
                Err(_) if active.load(Ordering::Acquire) == 0 => return Ok(()),
                Err(_) => continue,
            }
        } else {
            server.connect().await?;
        }
        if !windows_client_is_current_user(&server) {
            continue;
        }
        active.fetch_add(1, Ordering::AcqRel);
        let connection_active = active.clone();
        let root = root.clone();
        let service = service.clone();
        tokio::spawn(async move {
            let _ = serve_connection(server, root, service).await;
            connection_active.fetch_sub(1, Ordering::AcqRel);
        });
    }
}

#[cfg(windows)]
fn windows_client_is_current_user(pipe: &tokio::net::windows::named_pipe::NamedPipeServer) -> bool {
    use std::os::windows::io::AsRawHandle;
    use std::ptr;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        EqualSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    unsafe fn token_user(token: HANDLE) -> Option<Vec<usize>> {
        let mut required = 0;
        unsafe {
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut required);
        }
        if required == 0 {
            return None;
        }
        let words = (required as usize).div_ceil(std::mem::size_of::<usize>());
        let mut buffer = vec![0usize; words];
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                required,
                &mut required,
            )
        };
        (ok != 0).then_some(buffer)
    }

    unsafe {
        let mut client_pid = 0;
        if GetNamedPipeClientProcessId(pipe.as_raw_handle() as HANDLE, &mut client_pid) == 0 {
            return false;
        }
        let client_process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, client_pid);
        if client_process.is_null() {
            return false;
        }
        let mut client_token = ptr::null_mut();
        let mut server_token = ptr::null_mut();
        let opened_client = OpenProcessToken(client_process, TOKEN_QUERY, &mut client_token) != 0;
        let opened_server =
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut server_token) != 0;
        let same = if opened_client && opened_server {
            match (token_user(client_token), token_user(server_token)) {
                (Some(client), Some(server)) => {
                    let client = &*(client.as_ptr().cast::<TOKEN_USER>());
                    let server = &*(server.as_ptr().cast::<TOKEN_USER>());
                    EqualSid(client.User.Sid, server.User.Sid) != 0
                }
                _ => false,
            }
        } else {
            false
        };
        if !client_token.is_null() {
            CloseHandle(client_token);
        }
        if !server_token.is_null() {
            CloseHandle(server_token);
        }
        CloseHandle(client_process);
        same
    }
}

#[cfg(unix)]
type ClientStream = tokio::net::UnixStream;

#[cfg(unix)]
fn unix_endpoint(root: &Path) -> io::Result<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("notist-{}", unsafe { libc::geteuid() }))
        });
    Ok(runtime
        .join("notist")
        .join(format!("daemon-{}.sock", endpoint_discriminator(root))))
}

#[cfg(unix)]
async fn connect_stream(root: &Path) -> io::Result<ClientStream> {
    tokio::net::UnixStream::connect(unix_endpoint(root)?).await
}

#[cfg(unix)]
async fn serve_platform(
    root: PathBuf,
    service: Arc<NotistService>,
    idle_timeout: Option<Duration>,
) -> io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let endpoint = unix_endpoint(&root)?;
    let root = Arc::new(root);
    let directory = endpoint
        .parent()
        .ok_or_else(|| io::Error::other("daemon endpoint has no parent"))?;
    std::fs::create_dir_all(directory)?;
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    let metadata = std::fs::metadata(directory)?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "daemon runtime directory is not private to the current user",
        ));
    }
    if endpoint.exists() {
        if tokio::net::UnixStream::connect(&endpoint).await.is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "Notist daemon is already running",
            ));
        }
        std::fs::remove_file(&endpoint)?;
    }
    let listener = tokio::net::UnixListener::bind(&endpoint)?;
    let active = Arc::new(AtomicUsize::new(0));
    loop {
        let accepted = if let Some(idle_timeout) = idle_timeout {
            match tokio::time::timeout(idle_timeout, listener.accept()).await {
                Ok(result) => Some(result?),
                Err(_) if active.load(Ordering::Acquire) == 0 => None,
                Err(_) => continue,
            }
        } else {
            Some(listener.accept().await?)
        };
        let Some((stream, _)) = accepted else {
            break;
        };
        let credentials = stream.peer_cred()?;
        if credentials.uid() != unsafe { libc::geteuid() } {
            continue;
        }
        active.fetch_add(1, Ordering::AcqRel);
        let connection_active = active.clone();
        let root = root.clone();
        let service = service.clone();
        tokio::spawn(async move {
            let _ = serve_connection(stream, root, service).await;
            connection_active.fetch_sub(1, Ordering::AcqRel);
        });
    }
    drop(listener);
    std::fs::remove_file(endpoint)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ClientKind, ProtocolVersion};

    #[test]
    fn framing_rejects_oversized_messages_and_round_trips_json() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (mut left, mut right) = tokio::io::duplex(4096);
            let message = ClientMessage::Handshake {
                handshake: Handshake {
                    protocol_version: ProtocolVersion::CURRENT,
                    client_kind: ClientKind::Test,
                    client_version: "test".into(),
                    vault_root: PathBuf::from("/test"),
                    requested_capabilities: Vec::new(),
                },
            };
            write_frame(&mut left, &message).await.unwrap();
            let decoded: ClientMessage = read_frame(&mut right).await.unwrap();
            assert!(matches!(decoded, ClientMessage::Handshake { .. }));
        });
    }

    #[test]
    fn daemon_endpoint_and_handshake_are_scoped_to_one_vault() {
        let first = tempfile::TempDir::new().unwrap();
        let second = tempfile::TempDir::new().unwrap();
        let first_root = dunce::canonicalize(first.path()).unwrap();
        let second_root = dunce::canonicalize(second.path()).unwrap();
        assert_ne!(
            endpoint_discriminator(&first_root),
            endpoint_discriminator(&second_root)
        );

        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (mut client, server) = tokio::io::duplex(4096);
            let task = tokio::spawn(serve_connection(
                server,
                Arc::new(first_root),
                Arc::new(NotistService::new()),
            ));
            write_frame(
                &mut client,
                &ClientMessage::Handshake {
                    handshake: Handshake {
                        protocol_version: ProtocolVersion::CURRENT,
                        client_kind: ClientKind::Test,
                        client_version: "test".into(),
                        vault_root: second_root,
                        requested_capabilities: Vec::new(),
                    },
                },
            )
            .await
            .unwrap();
            let response: ServerMessage = read_frame(&mut client).await.unwrap();
            assert!(matches!(response, ServerMessage::HandshakeRejected { .. }));
            task.await.unwrap().unwrap();
        });
    }

    #[test]
    fn vault_scoped_daemon_exits_after_an_idle_grace_period() {
        let root = tempfile::TempDir::new().unwrap();
        let root = dunce::canonicalize(root.path()).unwrap();
        let service = Arc::new(NotistService::for_root(&root).unwrap());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime
            .block_on(serve(root, service, Some(Duration::from_millis(20))))
            .unwrap();
    }
}
