import { EditorDocument } from "../document/index.mjs";
export { IndexedDbStore, MemoryStore } from "./store.mjs";

function sameIdentity(a, b) { return a.document_id === b.document_id && a.history_id === b.history_id; }
function sameVersion(a, b) { return sameIdentity(a.identity, b.identity) && JSON.stringify(a.clocks) === JSON.stringify(b.clocks); }

function normalize(error) {
  if (typeof error !== "string") return error;
  try { const value = JSON.parse(error); return Object.assign(new Error(value.message || value.code), value); }
  catch { return new Error(error); }
}

// Browser I/O host for the Rust editor-node state machine. This owns no CRDT.
export class EditorNode {
  constructor({ NodeBinding, DocumentBinding, store, nodeId = store.nodeId, sessionId = crypto.randomUUID(), onError = console.error }) {
    this.binding = new NodeBinding(nodeId, sessionId);
    this.DocumentBinding = DocumentBinding; this.store = store;
    this.nodeId = nodeId; this.sessionId = sessionId; this.onError = onError;
    this.documents = new Map(); this.links = new Map(); this.connections = new Set();
    this.running = null; this.requested = false; this.closed = false; this.failure = null;
  }
  call(method, ...args) { try { return this.binding[method](...args); } catch (error) { throw normalize(error); } }
  async openDocument({ identity, credential, seed, text = "" }) {
    const id = identity.document_id;
    if (this.documents.has(id)) throw new Error("Document already open");
    const journal = await this.store.load(id);
    let raw;
    try {
      if (journal.length) {
        if (!sameIdentity(journal[0].packet.identity, identity)) throw new Error("Stored document history differs from requested history");
        raw = this.DocumentBinding.from_snapshot(JSON.stringify(journal[0].packet));
        for (const [index, entry] of journal.entries()) {
          if (entry.sequence !== index + 1 || !sameIdentity(entry.packet.identity, identity)) throw new Error("Corrupt document journal");
          if (index) raw.import_updates(JSON.stringify(entry.packet), "recovery");
          if (!sameVersion(JSON.parse(raw.snapshot()).version, entry.applied)) throw new Error("Journal version mismatch");
        }
      } else if (seed) {
        if (!sameIdentity(seed.identity, identity)) throw new Error("Seed history mismatch");
        raw = this.DocumentBinding.from_snapshot(JSON.stringify(seed));
      } else { raw = new this.DocumentBinding(JSON.stringify(identity), undefined, text); }
      this.call("attach_document", raw, credential, journal.at(-1)?.sequence, !!this.store.durable);
    } catch (error) { raw?.free(); throw normalize(error); }
    // Keep Document's mutation guards and immutable notifications intact while
    // routing imports through the journal, including causally pending packets.
    const managed = new Proxy(raw, { get: (target, method) => {
      if (method === "import_updates" || method === "import_binary") return (...args) => {
        const result = this.call(method === "import_updates" ? "import_packet" : "import_binary", ...args);
        this.schedule(); return result;
      };
      const value = target[method]; return typeof value === "function" ? value.bind(target) : value;
    } });
    const doc = new EditorDocument(managed, this.onError);
    const unsubscribe = doc.subscribe(() => this.schedule());
    this.documents.set(id, { doc, unsubscribe });
    await this.flush(); return doc;
  }
  document(id) { return this.documents.get(id)?.doc; }
  durableVersion(id) { return JSON.parse(this.call("durable_version", id)); }
  remoteDurableVersions(id) { return [...this.links.keys()].map(link => JSON.parse(this.call("remote_durable_version", link, id))).filter(Boolean); }
  flushEvents() { for (const { doc } of this.documents.values()) doc.flushEvents(); }
  addLink(id, send, close) { if (this.closed || this.failure) { close(); return; } this.call("connect", id); this.links.set(id, { send, close }); this.schedule(); }
  removeLink(id) { if (this.closed) return; this.call("disconnect", id); this.links.delete(id); }
  receive(id, text) {
    if (this.closed || !this.links.has(id)) return;
    try {
      if (text.length > 65536) throw new Error("Peer frame too large");
      this.call("receive", id, text); this.flushEvents(); this.schedule();
    } catch (error) { this.onError(error); this.links.get(id)?.close(); this.removeLink(id); }
  }
  schedule() {
    if (this.closed || this.failure) return;
    this.requested = true;
    if (!this.running) {
      this.running = Promise.resolve().then(() => this.pump()).catch(error => { this.failure = normalize(error); this.onError(this.failure); }).finally(() => { this.running = null; if (this.requested && !this.failure) this.schedule(); });
    }
  }
  async pump() {
    do {
      this.requested = false; this.call("poll"); this.flushEvents();
      const effects = JSON.parse(this.call("take_effects"));
      if (!effects.length) continue;
      for (const effect of effects) {
        if (effect.type === "persist") {
          await this.store.append(effect.document, effect.entry);
          this.call("stored", effect.document, effect.entry.sequence, !!this.store.durable);
        } else {
          const link = this.links.get(effect.link);
          if (link) try { await link.send(JSON.stringify(effect.message)); }
          catch { link.close(); this.removeLink(effect.link); }
        }
      }
      this.requested = true;
    } while (this.requested);
  }
  async flush() {
    this.schedule();
    while (this.running) await this.running;
    if (this.failure) throw this.failure;
  }
  async close() {
    if (this.closed) return;
    for (const connection of this.connections) connection.close();
    for (const link of this.links.values()) link.close();
    try { await this.flush(); }
    finally {
      this.closed = true;
      for (const { doc, unsubscribe } of this.documents.values()) { unsubscribe(); doc.dispose(); }
      this.binding.free(); await this.store.close();
    }
  }
}

export function connectWebSocket(node, { url, token }) {
  let socket, timer, stopped = false, link;
  const control = { close() { stopped = true; clearTimeout(timer); socket?.close(); if (link) node.removeLink(link); node.connections.delete(control); } };
  node.connections.add(control);
  function connect() {
    if (stopped || node.closed) return;
    socket = new WebSocket(url); let authenticated = false;
    socket.onopen = () => socket.send(JSON.stringify({ type: "authenticate", token }));
    socket.onmessage = event => {
      if (!authenticated) {
        if (JSON.parse(event.data).type !== "authenticated") { socket.close(); return; }
        authenticated = true; link = crypto.randomUUID(); const current = socket;
        node.addLink(link, text => { if (current.readyState !== WebSocket.OPEN || current.bufferedAmount > 1024 * 1024) throw new Error("WebSocket unavailable"); current.send(text); }, () => current.close());
      } else { node.receive(link, event.data); }
    };
    socket.onclose = () => { if (link) { node.removeLink(link); link = null; } if (!stopped) timer = setTimeout(connect, 1000); };
    socket.onerror = () => socket.close();
  }
  connect(); return control;
}

export function connectSignaling(node, { url, token, iceTransportPolicy = "all", turnTransport, onPeer = () => {} }) {
  let socket, retry, refresh, stopped = false, iceServers = [];
  const peers = new Map(); const online = new Map();
  const key = peer => JSON.stringify([peer.node, peer.session]);
  const ownKey = key({ node: node.nodeId, session: node.sessionId });
  const control = { peers, close() { stopped = true; clearTimeout(retry); clearInterval(refresh); socket?.close(); for (const peer of peers.values()) destroy(peer); peers.clear(); node.connections.delete(control); } };
  node.connections.add(control);
  function configure(servers) {
    iceServers = servers.map(server => ({ ...server, urls: server.urls.filter(url => !turnTransport || (turnTransport === "tls" ? url.startsWith("turns:") : url.startsWith("turn:") && url.endsWith(`transport=${turnTransport}`))) })).filter(s => s.urls.length);
  }
  function send(peer, data) {
    if (socket?.readyState !== WebSocket.OPEN) throw new Error("Signaling disconnected");
    socket.send(JSON.stringify({ type: "signal", to: peer.node, session: peer.session, data: { connection: peer.connection, ...data } }));
  }
  function destroy(peer) { clearTimeout(peer.retry); if (peer.link) node.removeLink(peer.link); peer.channel?.close(); peer.pc.close(); }
  function make(remote, connection = crypto.randomUUID()) {
    const id = key(remote); const old = peers.get(id); if (old) destroy(old);
    if (!old && peers.size >= 32) throw new Error("Peer connection limit reached");
    const pc = new RTCPeerConnection({ iceServers, iceTransportPolicy });
    const peer = { ...remote, id, connection, pc, link: null, channel: null, busy: false }; peers.set(id, peer);
    pc.ondatachannel = event => bind(peer, event.channel);
    pc.onconnectionstatechange = () => {
      onPeer(peer);
      if (pc.connectionState === "failed") {
        if (peer.link) { node.removeLink(peer.link); peer.link = null; }
        if (ownKey < id && online.has(id) && !stopped) peer.retry = setTimeout(() => initiate(remote), 1000);
      }
    };
    return peer;
  }
  function bind(peer, channel) {
    peer.channel = channel; channel.bufferedAmountLowThreshold = 128 * 1024;
    channel.onopen = () => {
      peer.link = crypto.randomUUID();
      node.addLink(peer.link, async text => {
        if (channel.bufferedAmount > 256 * 1024) await new Promise((resolve, reject) => {
          const timer = setTimeout(() => { cleanup(); reject(new Error("DataChannel backpressure timeout")); }, 10000);
          const cleanup = () => { clearTimeout(timer); channel.removeEventListener("bufferedamountlow", ready); channel.removeEventListener("close", closed); };
          const ready = () => { cleanup(); resolve(); }; const closed = () => { cleanup(); reject(new Error("DataChannel closed")); };
          channel.addEventListener("bufferedamountlow", ready); channel.addEventListener("close", closed);
        });
        if (channel.readyState !== "open") throw new Error("DataChannel is not open"); channel.send(text);
      }, () => peer.pc.close());
      onPeer(peer);
    };
    channel.onmessage = event => { if (peer.link && typeof event.data === "string") node.receive(peer.link, event.data); };
    channel.onclose = () => { if (peer.link) { node.removeLink(peer.link); peer.link = null; } onPeer(peer); };
  }
  async function gathered(pc) {
    if (pc.iceGatheringState === "complete") return;
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => { cleanup(); reject(new Error("ICE gathering timed out")); }, 15000);
      const cleanup = () => { clearTimeout(timer); pc.removeEventListener("icegatheringstatechange", changed); };
      const changed = () => { if (pc.iceGatheringState === "complete") { cleanup(); resolve(); } };
      pc.addEventListener("icegatheringstatechange", changed);
    });
  }
  async function offer(peer, restart = false) {
    if (peer.busy) return;
    peer.busy = true;
    try {
      await peer.pc.setLocalDescription(await peer.pc.createOffer({ iceRestart: restart })); await gathered(peer.pc);
      send(peer, { description: peer.pc.localDescription.toJSON() });
    } finally { peer.busy = false; }
  }
  async function initiate(remote) {
    if (stopped || !online.has(key(remote))) return;
    let peer;
    try { peer = make(remote); bind(peer, peer.pc.createDataChannel("notist-peer", { ordered: true })); await offer(peer); }
    catch (error) { if (!stopped) node.onError(error); if (peer) destroy(peer); }
  }
  async function message(body) {
    if (body.type === "registered") {
      configure(body.ice_servers); online.clear();
      for (const remote of body.peers) { const id = key(remote); online.set(id, remote); if (ownKey < id && !["connected", "connecting"].includes(peers.get(id)?.pc.connectionState)) void initiate(remote); }
      clearInterval(refresh); refresh = setInterval(() => { if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify({ type: "refresh_ice" })); }, 240000);
    } else if (body.type === "ice") {
      configure(body.ice_servers);
      for (const peer of peers.values()) { peer.pc.setConfiguration({ iceServers, iceTransportPolicy }); if (ownKey < peer.id && peer.pc.connectionState === "connected") await offer(peer, true); }
    } else if (body.type === "joined") {
      const id = key(body); online.set(id, body); if (ownKey < id) void initiate(body);
    } else if (body.type === "left") {
      online.delete(key(body)); // Established direct links remain usable without signaling.
    } else if (body.type === "signal") {
      const remote = { node: body.from, session: body.session }; const id = key(remote); const data = body.data;
      online.set(id, remote);
      if (!data?.description || typeof data.connection !== "string") return;
      let peer = peers.get(id);
      if (data.description.type === "offer") {
        if (ownKey < id) return; // Deterministic offerer also handles simultaneous discovery.
        if (!peer || peer.connection !== data.connection) peer = make(remote, data.connection);
        await peer.pc.setRemoteDescription(data.description);
        await peer.pc.setLocalDescription(await peer.pc.createAnswer()); await gathered(peer.pc);
        send(peer, { description: peer.pc.localDescription.toJSON() });
      } else if (data.description.type === "answer" && peer?.connection === data.connection) { await peer.pc.setRemoteDescription(data.description); }
    }
  }
  function connect() {
    if (stopped) return;
    socket = new WebSocket(url);
    socket.onopen = () => socket.send(JSON.stringify({ type: "register", node: node.nodeId, session: node.sessionId, token }));
    let chain = Promise.resolve();
    socket.onmessage = event => { chain = chain.then(() => message(JSON.parse(event.data))).catch(error => { if (!stopped) node.onError(error); }); };
    socket.onclose = () => { clearInterval(refresh); if (!stopped) retry = setTimeout(connect, 1000); };
    socket.onerror = () => socket.close();
  }
  connect(); return control;
}
