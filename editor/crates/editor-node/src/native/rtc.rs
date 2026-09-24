//! Native DataChannel endpoint using exactly the browser signaling messages and
//! the same PeerNode state machine as the WebSocket adapter.
use super::super::config::RemoteConfig;
use super::*;
use webrtc::{
    api::APIBuilder,
    data_channel::{
        RTCDataChannel, data_channel_message::DataChannelMessage,
        data_channel_state::RTCDataChannelState,
    },
    ice_transport::ice_server::RTCIceServer,
    peer_connection::{
        RTCPeerConnection, configuration::RTCConfiguration,
        peer_connection_state::RTCPeerConnectionState,
        sdp::session_description::RTCSessionDescription,
    },
};

struct Peer {
    node: String,
    session: String,
    connection: String,
    pc: Arc<RTCPeerConnection>,
    created: std::time::Instant,
}
fn peer_key(node: &str, session: &str) -> String {
    json!([node, session]).to_string()
}

async fn bind(app: Shared, channel: Arc<RTCDataChannel>) {
    let active = Arc::new(Mutex::new(None::<String>));
    let input_app = app.clone();
    let input_active = active.clone();
    let weak = Arc::downgrade(&channel);
    channel.on_message(Box::new(move |message: DataChannelMessage| {
        let (app, active, weak) = (input_app.clone(), input_active.clone(), weak.clone());
        Box::pin(async move {
            let link = active.lock().unwrap().clone();
            if let Some(link) = link {
                let result = std::str::from_utf8(&message.data)
                    .map_err(anyhow::Error::from)
                    .and_then(|text| receive(&app, &link, text));
                if let Err(error) = result {
                    app.error(error);
                    if let Some(channel) = weak.upgrade() {
                        let _ = channel.close().await;
                    }
                }
            }
        })
    }));
    let close_app = app.clone();
    let close_active = active.clone();
    channel.on_close(Box::new(move || {
        let (app, active) = (close_app.clone(), close_active.clone());
        Box::pin(async move {
            if let Some(link) = active.lock().unwrap().take() {
                remove_link(&app, &link);
            }
        })
    }));
    let weak = Arc::downgrade(&channel);
    channel.on_open(Box::new(move || {
        Box::pin(async move {
            let Some(channel) = weak.upgrade() else { return; };
            let Ok((link, mut receiver)) = add_link(&app) else { let _ = channel.close().await; return; };
            *active.lock().unwrap() = Some(link.clone());
            tokio::spawn(async move {
                loop {
                    let text = tokio::select! { _ = app.stop.cancelled() => break, text = receiver.recv() => match text { Some(text) => text, None => break } };
                    let send = async {
                        while channel.buffered_amount().await > 256 * 1024 {
                            if channel.ready_state() != RTCDataChannelState::Open { bail!("data channel closed"); }
                            tokio::time::sleep(Duration::from_millis(5)).await;
                        }
                        channel.send_text(text).await?; Ok::<_, anyhow::Error>(())
                    };
                    if !matches!(tokio::time::timeout(Duration::from_secs(10), send).await, Ok(Ok(()))) { break; }
                }
                remove_link(&app, &link); let _ = channel.close().await;
            });
        })
    }));
}
async fn make(
    app: Shared,
    node: String,
    session: String,
    connection: String,
    ice: &[RTCIceServer],
) -> Result<Peer> {
    let api = APIBuilder::new().build();
    let pc = Arc::new(
        api.new_peer_connection(RTCConfiguration {
            ice_servers: ice.to_vec(),
            ..Default::default()
        })
        .await?,
    );
    pc.on_data_channel(Box::new(move |channel| {
        let app = app.clone();
        Box::pin(async move {
            bind(app, channel).await;
        })
    }));
    Ok(Peer {
        node,
        session,
        connection,
        pc,
        created: std::time::Instant::now(),
    })
}
async fn description(
    peer: &Peer,
    outgoing: &mpsc::Sender<String>,
    offer: bool,
    restart: bool,
) -> Result<()> {
    let mut gathering = peer.pc.gathering_complete_promise().await;
    let description = if offer {
        peer.pc
            .create_offer(Some(
                webrtc::peer_connection::offer_answer_options::RTCOfferOptions {
                    ice_restart: restart,
                    ..Default::default()
                },
            ))
            .await?
    } else {
        peer.pc.create_answer(None).await?
    };
    peer.pc.set_local_description(description).await?;
    tokio::time::timeout(Duration::from_secs(15), gathering.recv()).await?;
    let description = peer
        .pc
        .local_description()
        .await
        .ok_or_else(|| anyhow::anyhow!("local SDP unavailable"))?;
    outgoing.send(json!({"type":"signal","to":peer.node,"session":peer.session,"data":{"connection":peer.connection,"description":description}}).to_string()).await?;
    Ok(())
}
pub(super) async fn run(
    app: Shared,
    mut incoming: mpsc::Receiver<String>,
    outgoing: mpsc::Sender<String>,
) {
    let session = app
        .node
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .session_id()
        .to_owned();
    let own = peer_key(&app.node_id, &session);
    let mut peers: BTreeMap<String, Peer> = BTreeMap::new();
    let mut online: BTreeMap<String, Value> = BTreeMap::new();
    let mut retry = tokio::time::interval(Duration::from_secs(2));
    let mut ice: Vec<RTCIceServer> = Vec::new();
    let mut refresh = tokio::time::interval(Duration::from_secs(240));
    refresh.tick().await;
    loop {
        let input = tokio::select! {
            _ = app.stop.cancelled() => break,
            _ = retry.tick() => json!({"type":"retry"}).to_string(),
            _ = refresh.tick() => { let _ = outgoing.send(json!({"type":"refresh_ice"}).to_string()).await; continue; },
            input = incoming.recv() => match input { Some(input) => input, None => break },
        };
        let result: Result<()> = async {
            let body: Value = serde_json::from_str(&input)?;
            if body["type"] == "registered" || body["type"] == "ice" {
                ice = serde_json::from_value(body["ice_servers"].clone())?;
                if body["type"] == "ice" {
                    for (key, peer) in &peers {
                        peer.pc
                            .set_configuration(RTCConfiguration {
                                ice_servers: ice.clone(),
                                ..Default::default()
                            })
                            .await?;
                        if own < *key
                            && peer.pc.connection_state() == RTCPeerConnectionState::Connected
                        {
                            description(peer, &outgoing, true, true).await?;
                        }
                    }
                }
            }
            if body["type"] == "registered" {
                online.clear();
            }
            if body["type"] == "left"
                && let (Some(node), Some(session)) =
                    (body["node"].as_str(), body["session"].as_str())
            {
                online.remove(&peer_key(node, session));
            }
            let discovered = if body["type"] == "registered" {
                body["peers"].as_array().cloned().unwrap_or_default()
            } else if body["type"] == "joined" {
                vec![body.clone()]
            } else {
                vec![]
            };
            for remote in discovered {
                if let (Some(node), Some(session)) =
                    (remote["node"].as_str(), remote["session"].as_str())
                {
                    online.insert(peer_key(node, session), remote.clone());
                }
            }
            let stale: Vec<_> = peers
                .iter()
                .filter(|(key, p)| {
                    !online.contains_key(*key)
                        && p.pc.connection_state() != RTCPeerConnectionState::Connected
                })
                .map(|(key, _)| key.clone())
                .collect();
            for key in stale {
                if let Some(peer) = peers.remove(&key) {
                    peer.pc.close().await?;
                }
            }
            for remote in online.values() {
                let (Some(node), Some(session)) =
                    (remote["node"].as_str(), remote["session"].as_str())
                else {
                    continue;
                };
                let key = peer_key(node, session);
                if key == own || own > key || (peers.len() >= 32 && !peers.contains_key(&key)) {
                    continue;
                }
                if let Some(old) = peers.get(&key) {
                    if old.pc.connection_state() == RTCPeerConnectionState::Connected
                        || (matches!(
                            old.pc.connection_state(),
                            RTCPeerConnectionState::Connecting | RTCPeerConnectionState::New
                        ) && old.created.elapsed() < Duration::from_secs(30))
                    {
                        continue;
                    }
                    old.pc.close().await?;
                }
                let peer = make(
                    app.clone(),
                    node.into(),
                    session.into(),
                    uuid::Uuid::new_v4().to_string(),
                    &ice,
                )
                .await?;
                let channel = peer.pc.create_data_channel("notist-peer", None).await?;
                bind(app.clone(), channel).await;
                description(&peer, &outgoing, true, false).await?;
                peers.insert(key, peer);
            }
            if body["type"] == "signal" {
                let (Some(node), Some(session), Some(connection)) = (
                    body["from"].as_str(),
                    body["session"].as_str(),
                    body["data"]["connection"].as_str(),
                ) else {
                    return Ok(());
                };
                let key = peer_key(node, session);
                let value = &body["data"]["description"];
                if value["type"] == "offer" {
                    if own < key {
                        return Ok(());
                    }
                    if !peers.get(&key).is_some_and(|p| p.connection == connection) {
                        if peers.len() >= 32 && !peers.contains_key(&key) {
                            return Ok(());
                        }
                        if let Some(old) = peers.remove(&key) {
                            old.pc.close().await?;
                        }
                        peers.insert(
                            key.clone(),
                            make(
                                app.clone(),
                                node.into(),
                                session.into(),
                                connection.into(),
                                &ice,
                            )
                            .await?,
                        );
                    }
                    let peer = &peers[&key];
                    peer.pc
                        .set_remote_description(serde_json::from_value::<RTCSessionDescription>(
                            value.clone(),
                        )?)
                        .await?;
                    description(peer, &outgoing, false, false).await?;
                } else if value["type"] == "answer"
                    && let Some(peer) = peers.get(&key).filter(|p| p.connection == connection)
                {
                    peer.pc
                        .set_remote_description(serde_json::from_value(value.clone())?)
                        .await?;
                }
            }
            Ok(())
        }
        .await;
        if let Err(error) = result
            && !app.stop.is_cancelled()
        {
            app.error(format!("WebRTC: {error}"));
        }
    }
    for peer in peers.values() {
        let _ = peer.pc.close().await;
    }
}

pub(super) async fn external(app: Shared, remote: RemoteConfig) {
    use tokio_tungstenite::tungstenite::Message as WsMessage;
    let (input, incoming) = mpsc::channel(64);
    let (outgoing, mut output) = mpsc::channel::<String>(64);
    let mut manager = tokio::spawn(run(app.clone(), incoming, outgoing));
    let mut manager_finished = false;
    while !app.stop.is_cancelled() {
        let attempt: Result<()> = async {
            let (mut ws, _) = tokio::time::timeout(Duration::from_secs(10), tokio_tungstenite::connect_async(&remote.url)).await??;
            let session = app.node.as_ref().unwrap().lock().unwrap().session_id().to_owned();
            ws.send(WsMessage::Text(json!({"type":"register","node":app.node_id,"session":session,"token":remote.access_token}).to_string().into())).await?;
            loop { tokio::select! {
                _ = app.stop.cancelled() => break,
                _ = &mut manager => { manager_finished = true; return Ok(()); },
                message = output.recv() => match message { Some(text) => ws.send(WsMessage::Text(text.into())).await?, None => break },
                message = ws.next() => match message {
                    Some(Ok(WsMessage::Text(text))) => { if text.len() > MAX_FRAME_BYTES { bail!("signaling frame too large"); } input.send(text.to_string()).await?; },
                    Some(Ok(WsMessage::Ping(data))) => ws.send(WsMessage::Pong(data)).await?,
                    Some(Ok(WsMessage::Pong(_))) => {},
                    _ => break,
                }
            }}
            Ok(())
        }.await;
        if manager_finished {
            break;
        }
        if let Err(error) = attempt
            && !app.stop.is_cancelled()
        {
            app.error(error);
        }
        tokio::select! { _ = app.stop.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(1)) => {} }
    }
    drop(input);
    if !manager_finished {
        let _ = manager.await;
    }
}
