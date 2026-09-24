use super::{
    auth::{self, TurnAuth},
    config::Config,
    store::Store,
    text_file::{Statuses, TextFileStatus, TextFiles},
    turn_service::{TurnService, tls_server_config},
};
use crate::{Effect, MAX_FRAME_BYTES, PeerNode, WireMessage, document::Document};
use anyhow::{Result, bail};
use axum::{
    Json, Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::Write,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{sync::mpsc, task::JoinSet};
use tokio_util::sync::CancellationToken;

#[path = "rtc.rs"]
mod rtc;

type Shared = Arc<App>;
struct SignalSession {
    node: String,
    session: String,
    tx: mpsc::Sender<String>,
    stop: CancellationToken,
}
struct App {
    config: Config,
    node_id: String,
    node: Option<Arc<Mutex<PeerNode>>>,
    links: Mutex<BTreeMap<String, mpsc::Sender<String>>>,
    signals: Mutex<BTreeMap<String, SignalSession>>,
    turn_auth: Arc<TurnAuth>,
    ice_urls: Vec<String>,
    stop: CancellationToken,
    errors: Mutex<Vec<String>>,
    text_files: Statuses,
}
impl App {
    fn authorized(&self, headers: &HeaderMap) -> bool {
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|token| auth::equal(token, &self.config.access_token))
    }
    fn ice_servers(&self) -> Value {
        if self.ice_urls.is_empty() {
            return json!([]);
        }
        let (username, credential) = self.turn_auth.credentials();
        json!([{ "urls": self.ice_urls, "username": username, "credential": credential }])
    }
    fn error(&self, error: impl std::fmt::Display) {
        eprintln!("node: {error}");
        let mut errors = self.errors.lock().unwrap();
        if errors.len() == 32 {
            errors.remove(0);
        }
        errors.push(error.to_string());
    }
}

pub struct NodeRuntime {
    app: Shared,
    turn: Option<TurnService>,
    tasks: JoinSet<()>,
    pub address: SocketAddr,
}
impl NodeRuntime {
    pub async fn start(config: Config) -> Result<Self> {
        config.validate()?;
        std::fs::create_dir_all(&config.state_dir)?;
        let identity_path = config.state_dir.join("node-id");
        let node_id = match std::fs::read_to_string(&identity_path) {
            Ok(id) => {
                uuid::Uuid::parse_str(id.trim())?;
                id.trim().to_owned()
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let id = uuid::Uuid::new_v4().to_string();
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&identity_path)?;
                file.write_all(id.as_bytes())?;
                file.sync_all()?;
                id
            }
            Err(e) => return Err(e.into()),
        };
        let session = uuid::Uuid::new_v4().to_string();
        let (node, store) = if config.peer.enabled {
            let store = Arc::new(Store::open(&config.state_dir.join("documents.redb"))?);
            let mut node = PeerNode::new(node_id.clone(), session)?;
            for entry in &config.peer.documents {
                let (document, sequence) = match store.recover(&entry.identity.document_id)? {
                    Some((doc, sequence)) => {
                        if doc.identity() != &entry.identity {
                            bail!("configured document history differs from stored history");
                        }
                        (doc, Some(sequence))
                    }
                    None => (
                        Document::new(entry.identity.clone(), None, &entry.initial_text)?,
                        None,
                    ),
                };
                node.attach(
                    Arc::new(Mutex::new(document)),
                    entry.credential.clone(),
                    sequence.map(|seq| (seq, true)),
                )?;
            }
            // Initial snapshots are durable before any document is advertised.
            for effect in node.take_effects() {
                if let Effect::Persist { document, entry } = effect {
                    store.commit(&document, &entry)?;
                    node.persisted(&document, entry.sequence)?;
                }
            }
            (Some(Arc::new(Mutex::new(node))), Some(store))
        } else {
            (None, None)
        };
        let mut text_files = TextFiles::new(&config)?;
        let text_errors = match (&store, &node) {
            (Some(store), Some(node)) => text_files.sync(store, node),
            _ => Vec::new(),
        };
        // Bind the shared HTTP listener before starting any background service.
        let listener = std::net::TcpListener::bind(config.http.listen)?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let tls = config
            .http
            .tls
            .as_ref()
            .map(tls_server_config)
            .transpose()?;
        let turn_auth = Arc::new(TurnAuth::default());
        let turn = if config.network_service.enabled {
            Some(
                TurnService::start(
                    config.network_service.turn.clone().unwrap(),
                    config.http.tls.as_ref(),
                    turn_auth.clone(),
                )
                .await?,
            )
        } else {
            None
        };
        let mut ice_urls = Vec::new();
        if let Some(turn) = &turn {
            let host = &config.network_service.turn.as_ref().unwrap().public_host;
            ice_urls.push(format!("stun:{host}:{}", turn.udp_address.port()));
            ice_urls.push(format!(
                "turn:{host}:{}?transport=udp",
                turn.udp_address.port()
            ));
            ice_urls.push(format!(
                "turn:{host}:{}?transport=tcp",
                turn.tcp_address.port()
            ));
            if let Some(address) = turn.tls_address {
                ice_urls.push(format!("turns:{host}:{}?transport=tcp", address.port()));
            }
        }
        let app = Arc::new(App {
            config,
            node_id,
            node,
            links: Mutex::new(BTreeMap::new()),
            signals: Mutex::new(BTreeMap::new()),
            turn_auth,
            ice_urls,
            stop: CancellationToken::new(),
            errors: Mutex::new(Vec::new()),
            text_files: text_files.statuses.clone(),
        });
        for error in text_errors {
            app.error(error);
        }
        let mut router = Router::new()
            .route("/health", get(health))
            .route("/bootstrap", get(bootstrap));
        if app.node.is_some() {
            router = router
                .route("/document", post(document))
                .route("/peer", get(peer_upgrade));
        }
        if app.config.network_service.enabled {
            router = router.route("/signal", get(signal_upgrade));
        }
        let router = router
            .layer(tower_http::cors::CorsLayer::permissive())
            .with_state(app.clone());
        let mut tasks = JoinSet::new();
        let handle = axum_server::Handle::new();
        let shutdown = handle.clone();
        let stop = app.stop.clone();
        tasks.spawn(async move {
            stop.cancelled().await;
            shutdown.graceful_shutdown(Some(Duration::from_secs(5)));
        });
        let serve_app = app.clone();
        tasks.spawn(async move {
            let result: std::io::Result<()> = async {
                if let Some(tls) = tls {
                    axum_server::from_tcp_rustls(
                        listener,
                        axum_server::tls_rustls::RustlsConfig::from_config(tls),
                    )?
                    .handle(handle)
                    .serve(router.into_make_service())
                    .await
                } else {
                    axum_server::from_tcp(listener)?
                        .handle(handle)
                        .serve(router.into_make_service())
                        .await
                }
            }
            .await;
            if let Err(error) = result {
                serve_app.error(error);
                serve_app.stop.cancel();
            }
        });
        if let Some(store) = store {
            tasks.spawn(pump(app.clone(), store, text_files));
        }
        if let Some(peer_node) = &app.node
            && app.config.network_service.enabled
        {
            let (input, incoming) = mpsc::channel(64);
            let (outgoing, mut output) = mpsc::channel::<String>(64);
            let node = app.node_id.clone();
            let session = peer_node.lock().unwrap().session_id().to_owned();
            let key = json!([node, session]).to_string();
            app.signals.lock().unwrap().insert(
                key.clone(),
                SignalSession {
                    node: node.clone(),
                    session: session.clone(),
                    tx: input.clone(),
                    stop: app.stop.child_token(),
                },
            );
            input.try_send(
                json!({"type":"registered","peers":[],"ice_servers":app.ice_servers()}).to_string(),
            )?;
            tasks.spawn(rtc::run(app.clone(), incoming, outgoing));
            let app = app.clone();
            tasks.spawn(async move {
                loop { tokio::select! {
                    _ = app.stop.cancelled() => break,
                    message = output.recv() => {
                        let Some(message) = message else { break; };
                        let Ok(body) = serde_json::from_str::<Value>(&message) else { continue; };
                        if body["type"] == "refresh_ice" { let _ = input.send(json!({"type":"ice","ice_servers":app.ice_servers()}).to_string()).await; continue; }
                        let target = json!([body["to"],body["session"]]).to_string();
                        if let Some(peer) = app.signals.lock().unwrap().get(&target)
                            && peer.tx.try_send(json!({"type":"signal","from":node,"session":session,"data":body["data"]}).to_string()).is_err() { peer.stop.cancel(); }
                    }
                }}
                app.signals.lock().unwrap().remove(&key);
            });
        }
        for remote in app.config.peer.signaling.clone() {
            let app = app.clone();
            tasks.spawn(async move {
                rtc::external(app, remote).await;
            });
        }
        for remote in app.config.peer.connect.clone() {
            let app = app.clone();
            tasks.spawn(async move {
                while !app.stop.is_cancelled() {
                    if let Err(error) = dial(app.clone(), &remote.url, &remote.access_token).await { app.error(error); }
                    tokio::select! { _ = app.stop.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(1)) => {} }
                }
            });
        }
        Ok(Self {
            app,
            turn,
            tasks,
            address,
        })
    }
    pub fn peer(&self) -> Option<Arc<Mutex<PeerNode>>> {
        self.app.node.clone()
    }
    pub fn info(&self) -> Value {
        json!({"node": self.app.node_id, "address": self.address, "network_service": self.app.config.network_service.enabled, "peer": self.app.node.is_some(), "ice_urls": self.app.ice_urls, "text_files": self.text_files()})
    }
    /// Text output has its own status; a durable CRDT receipt only acknowledges
    /// the journal and does not promise that a text file has been updated.
    pub fn text_files(&self) -> Vec<TextFileStatus> {
        self.app
            .text_files
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect()
    }
    /// Completes on a fatal runtime error or requested cancellation.
    pub async fn stopped(&self) {
        self.app.stop.cancelled().await;
    }
    pub async fn shutdown(mut self) -> Result<()> {
        self.app.stop.cancel();
        while self.tasks.join_next().await.is_some() {}
        if let Some(turn) = self.turn.take() {
            turn.shutdown().await?;
        }
        if !self.app.errors.lock().unwrap().is_empty() {
            eprintln!("node stopped with recorded errors; inspect health/logs");
        }
        Ok(())
    }
}

async fn health(State(app): State<Shared>) -> Json<Value> {
    let text_files: Vec<_> = app.text_files.lock().unwrap().values()
        .map(|status| json!({"document": status.document, "state": status.state, "version": status.version}))
        .collect();
    Json(
        json!({"ready": !app.stop.is_cancelled(), "network_service": app.config.network_service.enabled, "peer": app.node.is_some(), "errors": app.errors.lock().unwrap().clone(), "text_files": text_files}),
    )
}
async fn bootstrap(State(app): State<Shared>, headers: HeaderMap) -> impl IntoResponse {
    if !app.authorized(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"unauthorized"})),
        );
    }
    (
        StatusCode::OK,
        Json(
            json!({"node": app.node_id, "network_service": app.config.network_service.enabled, "peer": app.node.is_some(), "ice_servers": app.ice_servers()}),
        ),
    )
}
async fn document(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if !app.authorized(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"unauthorized"})),
        );
    }
    let Some(node) = &app.node else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error":"peer disabled"})),
        );
    };
    let id = body["document"].as_str().unwrap_or("");
    if !app.config.peer.documents.iter().any(|d| {
        d.identity.document_id == id
            && auth::equal(&d.credential, body["credential"].as_str().unwrap_or(""))
    }) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"document denied"})),
        );
    }
    let result = node.lock().unwrap().document(id).and_then(|doc| {
        doc.lock()
            .map_err(|_| crate::NodeError::Poisoned)?
            .export_snapshot()
            .map_err(Into::into)
    });
    match result {
        Ok(packet) => (StatusCode::OK, Json(json!(packet))),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"document unavailable"})),
        ),
    }
}
async fn signal_upgrade(
    State(app): State<Shared>,
    ws: WebSocketUpgrade,
) -> axum::response::Response {
    if !app.config.network_service.enabled {
        return StatusCode::NOT_FOUND.into_response();
    }
    ws.max_message_size(MAX_FRAME_BYTES)
        .max_frame_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| signal_socket(app, socket))
}
async fn peer_upgrade(State(app): State<Shared>, ws: WebSocketUpgrade) -> axum::response::Response {
    if app.node.is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    ws.max_message_size(MAX_FRAME_BYTES)
        .max_frame_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| peer_socket(app, socket))
}
async fn first(socket: &mut WebSocket) -> Option<Value> {
    match tokio::time::timeout(Duration::from_secs(5), socket.recv())
        .await
        .ok()??
        .ok()?
    {
        Message::Text(text) => serde_json::from_str(&text).ok(),
        _ => None,
    }
}

async fn signal_socket(app: Shared, mut socket: WebSocket) {
    let Some(registration) = first(&mut socket).await else {
        return;
    };
    if registration["type"] != "register"
        || !auth::equal(
            registration["token"].as_str().unwrap_or(""),
            &app.config.access_token,
        )
    {
        return;
    }
    let (Some(node), Some(session)) = (
        registration["node"].as_str(),
        registration["session"].as_str(),
    ) else {
        return;
    };
    if node.is_empty() || session.is_empty() || node.len() > 128 || session.len() > 128 {
        return;
    }
    let key = json!([node, session]).to_string();
    let (tx, mut rx) = mpsc::channel::<String>(64);
    let stop = app.stop.child_token();
    {
        let mut sessions = app.signals.lock().unwrap();
        if sessions.len() >= 256 || sessions.contains_key(&key) {
            return;
        }
        let peers: Vec<_> = sessions
            .values()
            .map(|s| json!({"node":s.node,"session":s.session}))
            .collect();
        let _ = tx.try_send(
            json!({"type":"registered", "peers":peers, "ice_servers":app.ice_servers()})
                .to_string(),
        );
        for other in sessions.values() {
            if other
                .tx
                .try_send(json!({"type":"joined", "node":node,"session":session}).to_string())
                .is_err()
            {
                other.stop.cancel();
            }
        }
        sessions.insert(
            key.clone(),
            SignalSession {
                node: node.into(),
                session: session.into(),
                tx,
                stop: stop.clone(),
            },
        );
    }
    loop {
        tokio::select! {
            _ = stop.cancelled() => break,
            message = rx.recv() => match message { Some(text) => if socket.send(Message::Text(text.into())).await.is_err() { break; }, None => break },
            incoming = socket.recv() => {
                let Some(Ok(message)) = incoming else { break; };
                match message {
                    Message::Text(text) => {
                        let Ok(body) = serde_json::from_str::<Value>(&text) else { break; };
                        if body["type"] == "refresh_ice" {
                            let text = json!({"type":"ice", "ice_servers":app.ice_servers()}).to_string();
                            if socket.send(Message::Text(text.into())).await.is_err() { break; }
                            continue;
                        }
                        if body["type"] != "signal" || !body["to"].is_string() || !body["session"].is_string() || text.len() > 32 * 1024 { break; }
                        let target = json!([body["to"],body["session"]]).to_string();
                        let sessions = app.signals.lock().unwrap();
                        if let Some(peer) = sessions.get(&target) {
                            let routed = json!({"type":"signal", "from":node,"session":session,"data":body["data"]}).to_string();
                            if peer.tx.try_send(routed).is_err() { peer.stop.cancel(); }
                        }
                    }
                    Message::Close(_) => break,
                    Message::Ping(data) => if socket.send(Message::Pong(data)).await.is_err() { break; },
                    _ => {}
                }
            }
        }
    }
    let mut sessions = app.signals.lock().unwrap();
    sessions.remove(&key);
    for peer in sessions.values() {
        if peer
            .tx
            .try_send(json!({"type":"left", "node":node,"session":session}).to_string())
            .is_err()
        {
            peer.stop.cancel();
        }
    }
}
fn add_link(app: &Shared) -> Result<(String, mpsc::Receiver<String>)> {
    let link = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = mpsc::channel(64);
    app.node
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .connect(link.clone())?;
    app.links.lock().unwrap().insert(link.clone(), tx);
    Ok((link, rx))
}
fn remove_link(app: &Shared, link: &str) {
    app.links.lock().unwrap().remove(link);
    app.node.as_ref().unwrap().lock().unwrap().disconnect(link);
}
fn receive(app: &Shared, link: &str, text: &str) -> Result<()> {
    if text.len() > MAX_FRAME_BYTES {
        bail!("peer frame too large");
    }
    let message: WireMessage = serde_json::from_str(text)?;
    app.node
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .receive(link, message)?;
    Ok(())
}
async fn peer_socket(app: Shared, mut socket: WebSocket) {
    let Some(body) = first(&mut socket).await else {
        return;
    };
    if body["type"] != "authenticate"
        || !auth::equal(
            body["token"].as_str().unwrap_or(""),
            &app.config.access_token,
        )
    {
        return;
    }
    if socket
        .send(Message::Text(
            json!({"type":"authenticated"}).to_string().into(),
        ))
        .await
        .is_err()
    {
        return;
    }
    let Ok((link, mut rx)) = add_link(&app) else {
        return;
    };
    loop {
        tokio::select! {
            _ = app.stop.cancelled() => break,
            message = rx.recv() => match message { Some(text) => if socket.send(Message::Text(text.into())).await.is_err() { break; }, None => break },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(text))) => if let Err(error) = receive(&app, &link, &text) { app.error(error); break; },
                Some(Ok(Message::Ping(data))) => if socket.send(Message::Pong(data)).await.is_err() { break; },
                Some(Ok(Message::Pong(_))) => {},
                _ => break,
            }
        }
    }
    remove_link(&app, &link);
}
async fn dial(app: Shared, url: &str, token: &str) -> Result<()> {
    use tokio_tungstenite::tungstenite::Message as WsMessage;
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async(url),
    )
    .await??;
    socket
        .send(WsMessage::Text(
            json!({"type":"authenticate","token":token})
                .to_string()
                .into(),
        ))
        .await?;
    let first = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await?
        .ok_or_else(|| anyhow::anyhow!("peer disconnected"))??;
    if serde_json::from_str::<Value>(first.to_text()?)?["type"] != "authenticated" {
        bail!("peer authentication failed");
    }
    let (link, mut rx) = add_link(&app)?;
    let result: Result<()> = async {
        loop {
            tokio::select! {
                _ = app.stop.cancelled() => break,
                message = rx.recv() => match message { Some(text) => socket.send(WsMessage::Text(text.into())).await?, None => break },
                incoming = socket.next() => match incoming {
                    Some(Ok(WsMessage::Text(text))) => receive(&app, &link, &text)?,
                    Some(Ok(WsMessage::Ping(data))) => socket.send(WsMessage::Pong(data)).await?,
                    Some(Ok(WsMessage::Pong(_))) => {},
                    _ => break,
                }
            }
        }
        Ok(())
    }.await;
    remove_link(&app, &link);
    result
}
async fn pump(app: Shared, store: Arc<Store>, mut text_files: TextFiles) {
    let mut last_text_sync = Instant::now();
    loop {
        let effects = {
            let mut node = app.node.as_ref().unwrap().lock().unwrap();
            if let Err(error) = node.poll() {
                app.error(error);
            }
            node.take_effects()
        };
        let empty = effects.is_empty();
        for effect in effects {
            match effect {
                Effect::Send { link, message } => {
                    if app.stop.is_cancelled() {
                        continue;
                    }
                    let sender = app.links.lock().unwrap().get(&link).cloned();
                    if let Some(sender) = sender {
                        let text = serde_json::to_string(&message).unwrap();
                        if !matches!(
                            tokio::time::timeout(Duration::from_secs(5), sender.send(text)).await,
                            Ok(Ok(()))
                        ) {
                            remove_link(&app, &link);
                        }
                    }
                }
                Effect::Persist { document, entry } => {
                    let store = store.clone();
                    let sequence = entry.sequence;
                    let id = document.clone();
                    // Commit in one ordered worker path; never block the async I/O threads.
                    match tokio::task::spawn_blocking(move || store.commit(&id, &entry)).await {
                        Ok(Ok(())) => {
                            if let Err(error) = app
                                .node
                                .as_ref()
                                .unwrap()
                                .lock()
                                .unwrap()
                                .persisted(&document, sequence)
                            {
                                app.error(error);
                                app.stop.cancel();
                            }
                        }
                        result => {
                            app.error(format!("storage commit failed: {result:?}"));
                            app.stop.cancel();
                            return;
                        }
                    }
                }
            }
        }
        let drained = app.stop.is_cancelled() && empty;
        // Coalesce rapid edits, but always flush after the journal is drained
        // on shutdown. Text I/O runs without holding a node/document lock.
        if !text_files.is_empty()
            && (drained || last_text_sync.elapsed() >= Duration::from_millis(100))
        {
            let store = store.clone();
            let node = app.node.as_ref().unwrap().clone();
            match tokio::task::spawn_blocking(move || {
                let errors = if drained {
                    text_files.flush(&store, &node)
                } else {
                    text_files.sync(&store, &node)
                };
                (text_files, errors)
            })
            .await
            {
                Ok((writer, errors)) => {
                    text_files = writer;
                    for error in errors {
                        app.error(error);
                    }
                }
                Err(error) => {
                    app.error(format!("text_file worker failed: {error}"));
                    app.stop.cancel();
                    return;
                }
            }
            last_text_sync = Instant::now();
        }
        if drained {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
