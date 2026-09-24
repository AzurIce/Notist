import { EditorNode, IndexedDbStore, connectSignaling, connectWebSocket } from "./index.mjs";

// Adapter for the source/rich-text prototype; the reusable node has no UI code.
export async function openNodeDocument({ NodeBinding, DocumentBinding, endpoint, documentId, profile, transport = "webrtc" }) {
  const credentials = new URLSearchParams(location.hash.slice(1));
  const token = credentials.get("token"), credential = credentials.get("credential");
  if (!token || !credential) throw new Error("节点连接需要 URL fragment 中的 token 和 credential。");
  const store = await IndexedDbStore.open(`${endpoint}/${documentId}/${profile}`);
  let node;
  try {
    const journal = await store.load(documentId);
    let seed = journal[0]?.packet;
    if (!seed) {
      const response = await fetch(new URL("document", endpoint.endsWith("/") ? endpoint : endpoint + "/"), {
        method: "POST", headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
        body: JSON.stringify({ document: documentId, credential }), signal: AbortSignal.timeout(10000),
      });
      if (!response.ok) throw new Error(`无法取得文档：HTTP ${response.status}`);
      seed = await response.json();
    }
    const errors = [];
    node = new EditorNode({ NodeBinding, DocumentBinding, store, onError(error) { errors.push(String(error)); console.error(error); } });
    const core = await node.openDocument({ identity: seed.identity, seed, credential });
    const offlineKey = `notist-node-offline:${endpoint}/${documentId}/${profile}`;
    let connection, timer, manualOffline = sessionStorage.getItem(offlineKey) === "true";
    const connect = () => {
      const url = new URL(transport === "websocket" ? "peer" : "signal", endpoint.endsWith("/") ? endpoint : endpoint + "/");
      url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
      connection = transport === "websocket" ? connectWebSocket(node, { url: url.href, token }) : connectSignaling(node, { url: url.href, token });
    };
    if (!manualOffline) connect();
    const session = {
      core, node,
      flush: () => node.flush(),
      inspect: () => ({ joined: node.links.size > 0, connection: node.links.size ? "connected" : "connecting", manualOffline,
        saved: JSON.stringify(node.durableVersion(documentId)) === JSON.stringify(core.snapshot().version),
        identity: seed.identity, writer: core.writerId, errors: [...errors], peers: node.links.size }),
      async setOffline(value) {
        manualOffline = value; sessionStorage.setItem(offlineKey, String(value));
        if (value) { connection?.close(); connection = null; } else if (!connection) connect();
        await node.flush();
      },
      mount() {
        const status = document.querySelector(".local-status");
        const button = document.createElement("button"); button.className = "quiet"; button.id = "network-toggle";
        button.onclick = () => session.setOffline(!manualOffline).catch(error => errors.push(String(error)));
        status.after(button); document.querySelector(".file-path").textContent = `${documentId} / ${profile}`;
        const refresh = () => {
          const state = session.inspect();
          status.textContent = errors.length ? errors.at(-1) : `${state.saved ? "已保存到此设备" : "保存中"} · ${manualOffline ? "离线" : state.peers ? `${state.peers} 个节点已连接` : "等待连接"}`;
          button.textContent = manualOffline ? "重新连接" : "离线编辑";
        };
        refresh(); timer = setInterval(refresh, 200);
      },
      async close() { clearInterval(timer); connection?.close(); await node.close(); },
    };
    return session;
  } catch (error) { if (node) await node.close().catch(() => {}); else await store.close(); throw error; }
}
