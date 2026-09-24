import { connectCore } from "./connection.mjs";
import { openStore } from "./store.mjs";

export async function openSyncDocument({ EditorDocument, DocumentBinding, room, replica }) {
  const store = await openStore(`${room}/${replica}`);
  let record = await store.read();
  if (!record) {
    const response = await fetch(`/sync/bootstrap?room=${encodeURIComponent(room)}`, { signal: AbortSignal.timeout(5000) });
    if (!response.ok) throw new Error(await response.text());
    record = { ...await response.json(), pending: [] };
    await store.write(record);
  } else {
    // Refresh the endpoint if the local experiment server changed its WS port.
    // Local history remains available even if this request fails.
    try {
      const response = await fetch(`/sync/bootstrap?room=${encodeURIComponent(room)}`, { signal: AbortSignal.timeout(1500) });
      if (response.ok) {
        const remote = await response.json();
        if (remote.identity.history_id !== record.identity.history_id) throw new Error("服务端历史已改变，本地草稿仍保留，请使用新的实验房间。");
        record.url = remote.url;
      }
    } catch (error) { if (error.message.includes("历史")) throw error; }
  }
  const core = EditorDocument.restore(DocumentBinding, record.packet);
  const pending = [...(record.pending || [])];
  for (const bytes of pending) core.importBinary(record.identity, Uint8Array.from(bytes), "local-recovery");
  let saveChain = Promise.resolve(), savedRevision = -1, generation = 0, saving = false, stopped = false;
  let storageError = null, networkError = null;
  let network = { connection: "connecting", joined: false }, manualOffline = record.offline || false;
  const errors = [], listeners = new Set();
  const notify = () => { for (const listener of listeners) listener(); };
  const fail = error => { networkError = String(error); errors.push(networkError); notify(); };
  function save() {
    if (stopped) return saveChain;
    const snapshot = core.snapshot();
    const request = ++generation;
    const next = { ...record, packet: core.exportSnapshot(), pending: pending.map(bytes => [...bytes]), offline: manualOffline };
    saving = true; notify();
    saveChain = saveChain.catch(() => {}).then(() => store.write(next)).then(() => {
      savedRevision = snapshot.revision;
      saving = request !== generation || savedRevision !== core.snapshot().revision;
      storageError = null;
      notify();
    });
    saveChain.catch(error => { storageError = String(error); errors.push(storageError); notify(); });
    return saveChain;
  }
  const stopObserve = core.subscribe(() => { save(); });
  const connection = connectCore(core, { url: record.url, roomId: record.roomId,
    onState(state) { network = state; if (state.joined) networkError = null; notify(); }, onError: fail,
    onImport(bytes, result) {
      if (result.pending) {
        // Keep causally premature packets across reload; snapshots alone do not
        // promise to contain Loro's pending queue. Duplicate replay is harmless.
        pending.push(Array.from(bytes));
        save();
      }
    },
  });
  if (manualOffline) connection.disconnect();
  await save();
  const session = {
    core, connection, flush: save,
    inspect: () => ({ ...network, manualOffline, saved: !saving && savedRevision === core.snapshot().revision, errors: [...errors], pendingPackets: pending.length, identity: record.identity, writer: core.writerId }),
    async setOffline(value) {
      manualOffline = value;
      if (value) connection.disconnect(); else connection.reconnect().catch(fail);
      await save(); notify();
    },
    mount() {
      const status = document.querySelector(".local-status");
      const button = document.createElement("button"); button.className = "quiet"; button.id = "network-toggle";
      button.onclick = () => session.setOffline(!manualOffline);
      status.after(button);
      document.querySelector(".file-path").textContent = `${room} / 副本 ${replica}`;
      document.querySelector(".experiment").textContent = "同步实验";
      const render = () => {
        const state = session.inspect();
        status.textContent = `${state.saved ? "本地已保存" : "正在保存"} · ${manualOffline ? "离线编辑" : network.joined ? "已连接" : "连接中"}`;
        status.title = storageError || networkError || "本地保存至 IndexedDB；服务端定期保存，连接成功不代表服务端已落盘。";
        if (storageError) status.textContent = "本地保存失败 · 请导出草稿";
        else if (networkError && !manualOffline) status.textContent = "暂时无法同步 · 可继续编辑";
        button.textContent = manualOffline ? "恢复连接" : "模拟断网";
        document.querySelector("#sync-status").textContent = `副本 ${replica} · ${manualOffline ? "离线修改将在重连后交换" : "Loro WebSocket · 服务端定期保存"}`;
      };
      listeners.add(render); render();
    },
    async destroy() { await save(); stopped = true; stopObserve(); connection.destroy(); store.close(); core.dispose(); },
  };
  return session;
}
