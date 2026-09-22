import { createServer } from "node:http";
import { createServer as createTCPServer } from "node:net";
import { readFile, writeFile, rename, mkdir } from "node:fs/promises";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
import { randomUUID } from "node:crypto";
import { SimpleServer } from "loro-websocket/server";
import { CrdtType } from "loro-protocol";
import { EditorCore } from "../../core/index.mjs";
import { serveProjection } from "../source-projection/server.mjs";

const root = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
const { EditorDocument } = require("../../scripts/pkg-core-node/notist_editor_core_wasm.js");
export const SAMPLE = "= 一起写下去\n\nHello world.\n\n你可以在任意一侧编辑；另一侧会收到相同的修改。\n\n== 试试离线\n\n断开一个副本，在两边分别输入，再恢复连接。\n\n// 源码、空行和注释仍然是文档的一部分。\n";

export async function freePort() {
  const socket = createTCPServer();
  await new Promise(resolve => socket.listen(0, "127.0.0.1", resolve));
  const port = socket.address().port;
  await new Promise(resolve => socket.close(resolve));
  return port;
}

// This is the unmodified official SimpleServer with its documented load/save
// hooks. Saves are periodic, not a durable ACK implementation.
export async function startExperiment({ port = 4175, wsPort = 0, directory = resolve(root, "data"), saveInterval = 500, beforeSave } = {}) {
  await mkdir(directory, { recursive: true });
  const records = new Map(), loading = new Map(), saving = new Map();
  const stats = { saves: 0, errors: [] };
  const pathFor = name => resolve(directory, `${name}.json`);
  async function writeRecord(name, record) {
    const previous = saving.get(name) || Promise.resolve();
    const task = previous.catch(() => {}).then(async () => {
      const temporary = `${pathFor(name)}.${randomUUID()}.tmp`;
      await writeFile(temporary, JSON.stringify(record));
      await rename(temporary, pathFor(name));
    });
    saving.set(name, task);
    await task;
  }
  function load(name) {
    if (!/^[a-zA-Z0-9_-]{1,40}$/.test(name)) throw new Error("Invalid room name");
    if (!loading.has(name)) loading.set(name, (async () => {
      let record;
      try { record = JSON.parse(await readFile(pathFor(name), "utf8")); }
      catch (error) {
        if (error.code !== "ENOENT") throw error;
        const identity = { document_id: `sync-lab/${name}`, history_id: randomUUID() };
        const doc = EditorCore.create(EditorDocument, { identity, text: SAMPLE });
        record = { identity, roomId: `${name}~${identity.history_id}`, packet: doc.exportSnapshot() };
        doc.dispose(); await writeRecord(name, record);
      }
      records.set(name, record);
      return record;
    })());
    return loading.get(name).then(() => records.get(name));
  }
  async function forRoom(roomId) {
    const record = await load(roomId.split("~")[0]);
    if (record.roomId !== roomId) throw new Error("Document history mismatch");
    return record;
  }
  const websocketPort = wsPort || await freePort();
  const server = new SimpleServer({ port: websocketPort, host: "127.0.0.1", saveInterval,
    async authenticate(roomId, crdt, auth) {
      try {
        const record = await forRoom(roomId), identity = JSON.parse(new TextDecoder().decode(auth));
        return crdt === CrdtType.Loro && identity.document_id === record.identity.document_id && identity.history_id === record.identity.history_id ? "write" : null;
      } catch { return null; }
    },
    async onLoadDocument(roomId) { return Uint8Array.from((await forRoom(roomId)).packet.data); },
    async onSaveDocument(roomId, _crdt, data) {
      const name = roomId.split("~")[0], previous = await forRoom(roomId);
      await beforeSave?.(data);
      const record = { ...previous, packet: { identity: previous.identity, kind: "snapshot", data: Array.from(data) } };
      await writeRecord(name, record);
      records.set(name, record); stats.saves++;
    },
  });
  await server.start();
  const http = createServer(async (req, res) => {
    const url = new URL(req.url, "http://localhost");
    try {
      if (url.pathname === "/sync/bootstrap") {
        const record = await load(url.searchParams.get("room") || "demo");
        res.writeHead(200, { "Content-Type": "application/json", "Cache-Control": "no-store" });
        res.end(JSON.stringify({ ...record, url: `ws://127.0.0.1:${websocketPort}` }));
      } else if (url.pathname === "/" || url.pathname === "/sync/client.js") {
        const path = url.pathname === "/" ? "index.html" : "dist/client.js";
        res.writeHead(200, { "Content-Type": path.endsWith("html") ? "text/html; charset=utf-8" : "text/javascript", "Cache-Control": "no-store" });
        res.end(await readFile(resolve(root, path)));
      } else {
        if (url.pathname === "/editor") req.url = "/";
        await serveProjection(req, res);
      }
    } catch (error) { res.writeHead(400); res.end(error.message); }
  });
  await new Promise(resolve => http.listen(port, "127.0.0.1", resolve));
  return { http, server, stats, load, url: `http://127.0.0.1:${http.address().port}`, wsUrl: `ws://127.0.0.1:${websocketPort}`,
    async stop() {
      await server.stop();
      // The reference server's stop() does not await asynchronous save hooks.
      // Wait for callbacks already enqueued by this experiment's file store.
      await new Promise(resolve => setImmediate(resolve));
      await Promise.all([...saving.values()]);
      await new Promise(resolve => http.close(resolve));
    },
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const experiment = await startExperiment({ port: Number(process.env.PORT || 4175), wsPort: Number(process.env.WS_PORT || 8787), directory: process.env.SYNC_DATA_DIR || resolve(root, "data") });
  console.log(`Loro sync experiment: ${experiment.url} (WebSocket ${experiment.wsUrl})`);
  for (const signal of ["SIGINT", "SIGTERM"]) process.once(signal, () => experiment.stop().finally(() => process.exit()));
}
