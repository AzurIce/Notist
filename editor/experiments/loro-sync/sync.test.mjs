import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { randomUUID } from "node:crypto";
import { setTimeout as delay } from "node:timers/promises";
import { WebSocket } from "ws";
import { SimpleServer } from "loro-websocket/server";
import { decode, MessageType } from "loro-protocol";
import { EditorDocument } from "../../document/index.mjs";
import { connectCore } from "./connection.mjs";
import { KernelAdaptor } from "./adaptor.mjs";
import { freePort, stopReferenceServer } from "./server.mjs";

globalThis.WebSocket = WebSocket;
const require = createRequire(import.meta.url);
const { DocumentBinding } = require("../../scripts/pkg-editor-node/notist_editor_node_wasm.js");
export async function waitFor(predicate, message = "condition", timeout = 8000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) { if (await predicate()) return; await delay(15); }
  throw new Error(`Timed out: ${message}`);
}
const append = (core, insert) => {
  const before = core.snapshot();
  core.transact({ expectedVersion: before.version, origin: "test", edits: [{ from: before.text.length, to: before.text.length, insert }] });
};
const equal = cores => cores.every(core => JSON.stringify(core.snapshot().version) === JSON.stringify(cores[0].snapshot().version));

async function fixture(t, options = {}) {
  const identity = { document_id: "network-test", history_id: randomUUID() };
  const base = EditorDocument.create(DocumentBinding, { identity, text: "Hello 世界 🧠\n" });
  const seed = base.exportSnapshot(); base.dispose();
  const port = await freePort(), roomId = `test-${identity.history_id}`;
  const errors = [], cores = [], connections = [], acks = [];
  let persisted = Uint8Array.from(seed.data), saves = 0;
  const config = { port, host: "127.0.0.1", saveInterval: options.saveInterval || 50,
    authenticate: async (_room, _crdt, auth) => JSON.parse(new TextDecoder().decode(auth)).history_id === identity.history_id ? "write" : null,
    onLoadDocument: async () => persisted,
    onSaveDocument: async (_room, _crdt, data) => {
      await options.beforeSave?.(data);
      persisted = data.slice(); saves++;
    },
  };
  let server = new SimpleServer(config); await server.start();
  t.after(async () => {
    for (const connection of connections) connection.destroy();
    await stopReferenceServer(server);
    for (const core of cores) core.dispose();
  });
  async function join(packet = seed) {
    const core = EditorDocument.restore(DocumentBinding, packet); cores.push(core);
    const connection = connectCore(core, { url: `ws://127.0.0.1:${port}`, roomId, onError: error => errors.push(String(error)) });
    connections.push(connection);
    connection.client.socket.addEventListener("message", ({ data }) => {
      if (typeof data === "string") return;
      const message = decode(new Uint8Array(data));
      if (message.type === MessageType.Ack) acks.push(message);
    });
    await connection.ready;
    await connection.adaptor.waitForReachingServerVersion();
    return { core, connection };
  }
  return { identity, seed, errors, acks, cores, connections, join,
    get saves() { return saves; },
    get persisted() { return persisted; },
    readSaved() {
      const doc = EditorDocument.restore(DocumentBinding, { ...seed, data: Array.from(persisted) });
      const snapshot = doc.snapshot(); doc.dispose(); return snapshot;
    },
    async restart() { await stopReferenceServer(server); server = new SimpleServer(config); await server.start(); },
  };
}

test("official WebSocket server converges three kernel replicas and preserves remote edits on undo", { timeout: 20000 }, async t => {
  const f = await fixture(t), a = await f.join(), b = await f.join(), c = await f.join();
  append(a.core, "来自 A😀\n"); append(b.core, "来自 B中文\n"); append(c.core, "来自 C\n");
  await waitFor(() => equal(f.cores), "three replicas converge");
  const merged = a.core.snapshot().text;
  for (const text of ["来自 A😀", "来自 B中文", "来自 C"]) assert.ok(merged.includes(text));
  a.core.undo(); await waitFor(() => equal(f.cores), "undo replicated");
  assert.ok(!b.core.snapshot().text.includes("来自 A😀"));
  assert.ok(b.core.snapshot().text.includes("来自 B中文"));
  a.core.redo(); await waitFor(() => equal(f.cores), "redo replicated");
  assert.equal(c.core.snapshot().text, merged);
  assert.deepEqual(f.errors, []);
});

test("both replicas edit offline and the official reconnect handshake exchanges both directions", { timeout: 20000 }, async t => {
  const f = await fixture(t), a = await f.join(), b = await f.join();
  a.connection.disconnect(); b.connection.disconnect();
  append(a.core, "离线 A 🐈\n"); append(b.core, "离线 B 🌿\n");
  assert.notEqual(a.core.snapshot().text, b.core.snapshot().text);
  await a.connection.reconnect(); await b.connection.reconnect();
  await waitFor(() => equal(f.cores), "offline fork merge");
  assert.ok(a.core.snapshot().text.includes("离线 A 🐈")); assert.ok(a.core.snapshot().text.includes("离线 B 🌿"));
  assert.deepEqual(f.errors, []);
});

test("restoring an offline CRDT snapshot uses a new writer and uploads unsent edits", { timeout: 20000 }, async t => {
  const f = await fixture(t), a = await f.join(), b = await f.join();
  a.connection.disconnect(); append(a.core, "关闭应用前的离线编辑\n");
  const snapshot = a.core.exportSnapshot(), oldWriter = a.core.writerId;
  a.connection.destroy();
  append(b.core, "另一台设备的新编辑\n");
  const recovered = await f.join(snapshot);
  assert.notEqual(recovered.core.writerId, oldWriter);
  await waitFor(() => equal([b.core, recovered.core]), "reload and remote merge");
  assert.ok(recovered.core.snapshot().text.includes("关闭应用前的离线编辑"));
  assert.ok(recovered.core.snapshot().text.includes("另一台设备的新编辑"));
  assert.deepEqual(f.errors, []);
});

test("server restart recovers a saved snapshot and accepts subsequent writes", { timeout: 20000 }, async t => {
  const f = await fixture(t), a = await f.join();
  append(a.core, "已经定期保存的文本\n");
  await waitFor(() => f.readSaved().text === a.core.snapshot().text, "periodic save");
  a.connection.destroy(); await f.restart();
  const b = await f.join();
  assert.equal(b.core.snapshot().text, a.core.snapshot().text);
  append(b.core, "重启后的修改\n");
  await waitFor(() => f.readSaved().text === b.core.snapshot().text, "save after restart");
  assert.deepEqual(f.errors, []);
});

test("raw binary adapter preserves core rejection and wrong-history joins are rejected", { timeout: 20000 }, async t => {
  const f = await fixture(t), a = await f.join();
  const before = a.core.snapshot();
  assert.deepEqual(a.core.decodeVersion(a.core.encodedVersion()), before.version);
  assert.throws(() => a.core.importBinary({ ...f.identity, history_id: "wrong" }, Uint8Array.from(f.seed.data)));
  assert.throws(() => a.connection.adaptor.applyUpdate([Uint8Array.from([1, 2, 3])]));
  assert.deepEqual(a.core.snapshot(), before);
  const wrong = EditorDocument.create(DocumentBinding, { identity: { ...f.identity, history_id: "wrong" }, text: "Unrelated" });
  const connection = connectCore(wrong, { url: a.connection.client.socket.url, roomId: `test-${f.identity.history_id}` });
  try { await assert.rejects(connection.ready, /auth|join|rejected/i); }
  finally { connection.destroy(); wrong.dispose(); }
});

test("reference server ACK confirms acceptance before any persistence callback", { timeout: 20000 }, async t => {
  const f = await fixture(t, { saveInterval: 60000 }), a = await f.join();
  append(a.core, "ACK 不等于落盘\n");
  await waitFor(() => f.acks.length > 0, "success ACK");
  assert.equal(f.acks.at(-1).status, 0);
  assert.equal(f.saves, 0);
  assert.ok(!f.readSaved().text.includes("ACK 不等于落盘"));
});

test("reproduces reference server losing dirty flag when edits arrive during async save", { timeout: 20000 }, async t => {
  let release, saving = false;
  const barrier = new Promise(resolve => { release = resolve; });
  const f = await fixture(t, { saveInterval: 300, beforeSave: async () => { saving = true; await barrier; } });
  t.after(() => release());
  const a = await f.join();
  append(a.core, "第一次修改\n");
  await waitFor(() => saving, "first save in flight");
  const ackCount = f.acks.length;
  append(a.core, "保存期间的新修改\n");
  await waitFor(() => f.acks.length > ackCount, "second edit accepted during save");
  release();
  await waitFor(() => f.saves === 1, "first save complete");
  await delay(700); // two further save intervals, with no new edits
  assert.equal(f.saves, 1, "upstream clears dirty although a newer update arrived");
  assert.ok(!f.readSaved().text.includes("保存期间的新修改"));
  assert.ok(a.core.snapshot().text.includes("保存期间的新修改"));
});

test("adapter does not import a second CRDT document and keeps pending updates replayable", () => {
  const identity = { document_id: "pending", history_id: randomUUID() };
  const a = EditorDocument.create(DocumentBinding, { identity, text: "" });
  const b = EditorDocument.restore(DocumentBinding, a.exportSnapshot());
  const start = a.snapshot().version; append(a, "A");
  const first = a.exportUpdatesSince(start), middle = a.snapshot().version; append(a, "B");
  const second = a.exportUpdatesSince(middle), pending = [];
  const adaptor = new KernelAdaptor(b, { onImport: (bytes, result) => { if (result.pending) pending.push(Array.from(bytes)); } });
  adaptor.applyUpdate([Uint8Array.from(second.data)]); assert.equal(pending.length, 1);
  assert.equal(b.snapshot().text, "");
  const restored = EditorDocument.restore(DocumentBinding, b.exportSnapshot());
  for (const bytes of pending) restored.importBinary(identity, Uint8Array.from(bytes));
  restored.import(first); assert.equal(restored.snapshot().text, "AB");
  adaptor.destroy(); a.dispose(); b.dispose(); restored.dispose();
});
