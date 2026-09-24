//! Embedded TURN. TCP/TLS framing adapts streams to the TURN crate's packet
//! interface; protocol parsing, allocations, permissions and authentication stay
//! in the upstream implementation.
use super::{
    auth::TurnAuth,
    config::{TlsConfig, TurnConfig},
};
use anyhow::{Result, bail};
use async_trait::async_trait;
use std::{io, net::SocketAddr, sync::Arc};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf},
    net::{TcpListener, UdpSocket},
    sync::{Mutex, OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
};
use tokio_rustls::{TlsAcceptor, rustls};
use tokio_util::sync::CancellationToken;
use turn::{
    relay::{RelayAddressGenerator, relay_range::RelayAddressGeneratorRanges},
    server::{
        Server,
        config::{ConnConfig, ServerConfig},
    },
};
use webrtc_util::{Conn, vnet::net::Net};

trait Io: AsyncRead + AsyncWrite + Send + Sync + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Sync + Unpin> Io for T {}
type Stream = Box<dyn Io>;
type IoResult<T> = std::result::Result<T, webrtc_util::Error>;
fn closed() -> webrtc_util::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, "TURN connection closed").into()
}

struct StreamConn {
    reader: Mutex<ReadHalf<Stream>>,
    writer: Mutex<WriteHalf<Stream>>,
    local: SocketAddr,
    remote: SocketAddr,
    stop: CancellationToken,
}
impl StreamConn {
    fn new(stream: Stream, local: SocketAddr, remote: SocketAddr, stop: CancellationToken) -> Self {
        let (reader, writer) = tokio::io::split(stream);
        Self {
            reader: Mutex::new(reader),
            writer: Mutex::new(writer),
            local,
            remote,
            stop,
        }
    }
    async fn read_packet(&self, buf: &mut [u8]) -> io::Result<usize> {
        let mut reader = self.reader.lock().await;
        let mut header = [0; 4];
        reader.read_exact(&mut header).await?;
        let length = u16::from_be_bytes([header[2], header[3]]) as usize;
        let channel = header[0] & 0xc0 == 0x40;
        if !channel && header[0] & 0xc0 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid TURN stream frame",
            ));
        }
        let total = length + if channel { 4 } else { 20 };
        if total > buf.len() || (!channel && !length.is_multiple_of(4)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "TURN frame exceeds supported MTU",
            ));
        }
        buf[..4].copy_from_slice(&header);
        reader.read_exact(&mut buf[4..total]).await?;
        if channel && !total.is_multiple_of(4) {
            let mut padding = [0; 3];
            reader.read_exact(&mut padding[..4 - total % 4]).await?;
        }
        Ok(total)
    }
}
#[async_trait]
impl Conn for StreamConn {
    async fn connect(&self, _: SocketAddr) -> IoResult<()> {
        Ok(())
    }
    async fn recv(&self, buf: &mut [u8]) -> IoResult<usize> {
        let result = tokio::select! { _ = self.stop.cancelled() => return Err(closed()), result = self.read_packet(buf) => result };
        if result.is_err() {
            self.stop.cancel();
        }
        Ok(result?)
    }
    async fn recv_from(&self, buf: &mut [u8]) -> IoResult<(usize, SocketAddr)> {
        Ok((self.recv(buf).await?, self.remote))
    }
    async fn send(&self, buf: &[u8]) -> IoResult<usize> {
        let send = async {
            let mut writer = self.writer.lock().await;
            writer.write_all(buf).await?;
            if buf.len() >= 4 && buf[0] & 0xc0 == 0x40 && !buf.len().is_multiple_of(4) {
                writer.write_all(&[0; 3][..4 - buf.len() % 4]).await?;
            }
            Ok::<_, io::Error>(buf.len())
        };
        tokio::select! { _ = self.stop.cancelled() => Err(closed()), result = tokio::time::timeout(std::time::Duration::from_secs(10), send) => Ok(result.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "TURN write timeout"))??) }
    }
    async fn send_to(&self, buf: &[u8], target: SocketAddr) -> IoResult<usize> {
        if target != self.remote {
            return Err(closed());
        }
        self.send(buf).await
    }
    fn local_addr(&self) -> IoResult<SocketAddr> {
        Ok(self.local)
    }
    fn remote_addr(&self) -> Option<SocketAddr> {
        Some(self.remote)
    }
    async fn close(&self) -> IoResult<()> {
        self.stop.cancel();
        let _ = self.writer.lock().await.shutdown().await;
        Ok(())
    }
    fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
        self
    }
}

struct RelayConn {
    inner: Arc<dyn Conn + Send + Sync>,
    _permit: OwnedSemaphorePermit,
    local_development: bool,
}
fn public_destination(address: SocketAddr) -> bool {
    match address.ip() {
        std::net::IpAddr::V4(ip) => {
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_multicast()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.octets()[0] == 0)
        }
        std::net::IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(|ip| public_destination(SocketAddr::new(ip.into(), address.port())))
            .unwrap_or_else(|| {
                !(ip.is_loopback()
                    || ip.is_unspecified()
                    || ip.is_multicast()
                    || ip.is_unique_local()
                    || ip.is_unicast_link_local())
            }),
    }
}
#[async_trait]
impl Conn for RelayConn {
    async fn connect(&self, address: SocketAddr) -> IoResult<()> {
        self.inner.connect(address).await
    }
    async fn recv(&self, buf: &mut [u8]) -> IoResult<usize> {
        self.inner.recv(buf).await
    }
    async fn recv_from(&self, buf: &mut [u8]) -> IoResult<(usize, SocketAddr)> {
        self.inner.recv_from(buf).await
    }
    async fn send(&self, buf: &[u8]) -> IoResult<usize> {
        self.inner.send(buf).await
    }
    async fn send_to(&self, buf: &[u8], target: SocketAddr) -> IoResult<usize> {
        if !self.local_development && !public_destination(target) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "private TURN destination",
            )
            .into());
        }
        self.inner.send_to(buf, target).await
    }
    fn local_addr(&self) -> IoResult<SocketAddr> {
        self.inner.local_addr()
    }
    fn remote_addr(&self) -> Option<SocketAddr> {
        self.inner.remote_addr()
    }
    async fn close(&self) -> IoResult<()> {
        self.inner.close().await
    }
    fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
        self
    }
}
struct RelayGenerator {
    inner: RelayAddressGeneratorRanges,
    slots: Arc<Semaphore>,
    local_development: bool,
}
#[async_trait]
impl RelayAddressGenerator for RelayGenerator {
    fn validate(&self) -> std::result::Result<(), turn::Error> {
        self.inner.validate()
    }
    async fn allocate_conn(
        &self,
        ipv4: bool,
        requested_port: u16,
    ) -> std::result::Result<(Arc<dyn Conn + Send + Sync>, SocketAddr), turn::Error> {
        if requested_port != 0
            && !(self.inner.min_port..=self.inner.max_port).contains(&requested_port)
        {
            return Err(turn::Error::ErrMaxRetriesExceeded);
        }
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| turn::Error::ErrMaxRetriesExceeded)?;
        let (inner, address) = self.inner.allocate_conn(ipv4, requested_port).await?;
        Ok((
            Arc::new(RelayConn {
                inner,
                _permit: permit,
                local_development: self.local_development,
            }),
            address,
        ))
    }
}

pub struct TurnService {
    server: Server,
    stop: CancellationToken,
    tasks: JoinSet<()>,
    pub udp_address: SocketAddr,
    pub tcp_address: SocketAddr,
    pub tls_address: Option<SocketAddr>,
}
impl TurnService {
    pub async fn start(
        config: TurnConfig,
        tls: Option<&TlsConfig>,
        auth: Arc<TurnAuth>,
    ) -> Result<Self> {
        let udp = Arc::new(UdpSocket::bind(config.listen_udp).await?);
        let tcp = TcpListener::bind(config.listen_tcp).await?;
        let tls_listener = match config.listen_tls {
            Some(address) => Some(TcpListener::bind(address).await?),
            None => None,
        };
        let acceptor = if tls_listener.is_some() {
            Some(TlsAcceptor::from(tls_server_config(tls.ok_or_else(
                || anyhow::anyhow!("TLS certificate missing"),
            )?)?))
        } else {
            None
        };
        let udp_address = udp.local_addr()?;
        let tcp_address = tcp.local_addr()?;
        let tls_address = tls_listener.as_ref().map(|l| l.local_addr()).transpose()?;
        let slots = Arc::new(Semaphore::new(256));
        let server = Server::new(server_config(udp, &config, auth.clone(), slots.clone())).await?;
        let stop = CancellationToken::new();
        let mut tasks = JoinSet::new();
        tasks.spawn(listen(
            tcp,
            None,
            config.clone(),
            auth.clone(),
            slots.clone(),
            stop.clone(),
        ));
        if let Some(listener) = tls_listener {
            tasks.spawn(listen(
                listener,
                acceptor,
                config,
                auth,
                slots,
                stop.clone(),
            ));
        }
        Ok(Self {
            server,
            stop,
            tasks,
            udp_address,
            tcp_address,
            tls_address,
        })
    }
    pub async fn shutdown(mut self) -> Result<()> {
        self.stop.cancel();
        self.server.close().await?;
        while self.tasks.join_next().await.is_some() {}
        Ok(())
    }
}
fn server_config(
    conn: Arc<dyn Conn + Send + Sync>,
    config: &TurnConfig,
    auth: Arc<TurnAuth>,
    slots: Arc<Semaphore>,
) -> ServerConfig {
    ServerConfig {
        conn_configs: vec![ConnConfig {
            conn,
            relay_addr_generator: Box::new(RelayGenerator {
                inner: RelayAddressGeneratorRanges {
                    relay_address: config.public_ip,
                    min_port: config.relay_ports[0],
                    max_port: config.relay_ports[1],
                    max_retries: 16,
                    address: config.listen_udp.ip().to_string(),
                    net: Arc::new(Net::new(None)),
                },
                slots,
                local_development: config.public_ip.is_loopback(),
            }),
        }],
        realm: "notist".into(),
        auth_handler: auth,
        channel_bind_timeout: Default::default(),
        alloc_close_notify: None,
    }
}
async fn listen(
    listener: TcpListener,
    acceptor: Option<TlsAcceptor>,
    config: TurnConfig,
    auth: Arc<TurnAuth>,
    slots: Arc<Semaphore>,
    stop: CancellationToken,
) {
    let connections = Arc::new(Semaphore::new(128));
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            _ = stop.cancelled() => break,
            _ = tasks.join_next(), if !tasks.is_empty() => {},
            accepted = listener.accept() => {
                let Ok((stream, remote)) = accepted else { break; };
                let Ok(permit) = connections.clone().try_acquire_owned() else { continue; };
                let local = match stream.local_addr() { Ok(a) => a, Err(_) => continue };
                let (acceptor, config, auth, slots, stop) = (acceptor.clone(), config.clone(), auth.clone(), slots.clone(), stop.child_token());
                tasks.spawn(async move {
                    let _permit = permit;
                    let stream: Stream = if let Some(acceptor) = acceptor {
                        match tokio::time::timeout(std::time::Duration::from_secs(10), acceptor.accept(stream)).await { Ok(Ok(stream)) => Box::new(stream), _ => return }
                    } else { Box::new(stream) };
                    let conn = Arc::new(StreamConn::new(stream, local, remote, stop.clone()));
                    if let Ok(server) = Server::new(server_config(conn, &config, auth, slots)).await {
                        stop.cancelled().await; let _ = server.close().await;
                    }
                });
            }
        }
    }
    stop.cancel();
    while tasks.join_next().await.is_some() {}
}
pub fn tls_server_config(config: &TlsConfig) -> Result<Arc<rustls::ServerConfig>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let certificates = rustls_pemfile::certs(&mut std::io::BufReader::new(std::fs::File::open(
        &config.certificate,
    )?))
    .collect::<std::result::Result<Vec<_>, _>>()?;
    let key = rustls_pemfile::private_key(&mut std::io::BufReader::new(std::fs::File::open(
        &config.private_key,
    )?))?;
    if certificates.is_empty() || key.is_none() {
        bail!("empty TLS certificate or key");
    }
    Ok(Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certificates, key.unwrap())?,
    ))
}
