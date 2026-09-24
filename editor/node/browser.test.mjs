import assert from "node:assert/strict";
import { chromium } from "playwright-core";
import { createServer } from "node:http";
import { readFile, writeFile, mkdtemp, readdir, mkdir, rm } from "node:fs/promises";
import { spawn, execFileSync } from "node:child_process";
import { tmpdir } from "node:os";
import { resolve, join, extname } from "node:path";
import { fileURLToPath } from "node:url";
import { startServer } from "../experiments/source-projection/server.mjs";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const directory = await mkdtemp(join(tmpdir(), "notist-node-browser-"));
const binary = resolve(root, "target/debug/notist-editor-node");
const token = "integration-test-access-token-123456";
const credential = "integration-test-document-token-123456";
const identity = { document_id: "note", history_id: "browser-history" };
const failures = [], reports = [], processes = new Set();
const server = createServer(async (req, res) => {
  try {
    const path = decodeURIComponent(new URL(req.url, "http://localhost").pathname);
    if (path === "/") {
      res.setHeader("Content-Type", "text/html");
      res.end(`<script type="module">
        import init, {NodeBinding, DocumentBinding} from '/pkg-editor/notist_editor_node_wasm.js';
        import {EditorNode, MemoryStore, IndexedDbStore, connectSignaling, connectWebSocket} from '/node/index.mjs';
        await init();
        window.boot = async options => {
          window.errors = [];
          const store = options.profile ? await IndexedDbStore.open(options.profile) : new MemoryStore();
          window.node = new EditorNode({NodeBinding, DocumentBinding, store, onError: e => errors.push(String(e))});
          window.doc = await node.openDocument(options);
          window.signal = options.signal ? connectSignaling(node, options.signal) : null;
          window.ws = options.ws ? connectWebSocket(node, options.ws) : null;
        };
        window.append = text => {const before = doc.snapshot(); const end = before.text.length; doc.transact({expectedVersion:before.version, edits:[{from:end,to:end,insert:text}]});};
        window.inspect = () => ({text:doc.snapshot().text,version:doc.snapshot().version, durable:node.durableVersion('note'), links:node.links.size, errors});
        window.ready = true;
      </script>`); return;
    }
    const file = resolve(root, "." + path);
    if (!file.startsWith(root + "/") || ![".mjs", ".js", ".wasm"].includes(extname(file))) throw new Error("not found");
    res.setHeader("Content-Type", extname(file) === ".wasm" ? "application/wasm" : "text/javascript"); res.end(await readFile(file));
  } catch { res.writeHead(404); res.end(); }
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
const origin = `http://127.0.0.1:${server.address().port}`;

async function start(name, { network = true, peer = false, tls = false, signaling } = {}) {
  const state = join(directory, name); await mkdir(state, { recursive: true });
  const certificate = join(directory, "cert.pem"), key = join(directory, "key.pem");
  if (tls) execFileSync("openssl", ["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1", "-keyout", key, "-out", certificate, "-subj", "/CN=localhost", "-addext", "subjectAltName=DNS:localhost,IP:127.0.0.1"], { stdio: "ignore" });
  const config = join(state, "node.toml");
  await writeFile(config, `state_dir = "${state}"
access_token = "${token}"
[http]
listen = "127.0.0.1:0"
public_url = "${tls ? "https" : "http"}://localhost"
${tls ? `[http.tls]\ncertificate = "${certificate}"\nprivate_key = "${key}"` : ""}
[network_service]
enabled = ${network}
${network ? `[network_service.turn]
listen_udp = "127.0.0.1:0"
listen_tcp = "127.0.0.1:0"
${tls ? 'listen_tls = "127.0.0.1:0"' : ""}
public_ip = "127.0.0.1"
public_host = "127.0.0.1"
relay_ports = [51000, 52000]` : ""}
[peer]
enabled = ${peer}
${signaling ? `[[peer.signaling]]\nurl = "${signaling}"\naccess_token = "${token}"` : ""}
${peer ? `[[peer.documents]]
credential = "${credential}"
initial_text = "Hello 🧠"
text_file = "note.not"
[peer.documents.identity]
document_id = "note"
history_id = "browser-history"` : ""}
`);
  const child = spawn(binary, ["--config", config], { stdio: ["ignore", "pipe", "pipe"] }); processes.add(child);
  let output = "", errors = "";
  child.stderr.on("data", data => { errors += data; });
  const info = await new Promise((resolveReady, reject) => {
    const timer = setTimeout(() => { child.kill("SIGKILL"); reject(new Error(`Node startup timeout: ${errors}`)); }, 15000);
    child.once("exit", code => { clearTimeout(timer); reject(new Error(`Node exited ${code}: ${errors}`)); });
    child.stdout.on("data", data => {
      output += data;
      const line = output.split("\n").find(line => line.startsWith('{"event":"ready"'));
      if (line) { clearTimeout(timer); resolveReady(JSON.parse(line).node); }
    });
  });
  return { child, state, info, url: `${tls ? "https" : "http"}://${info.address}`, ws: `${tls ? "wss" : "ws"}://${info.address}`, errors: () => errors,
    async stop(signal = "SIGTERM") { if (child.exitCode !== null || child.signalCode) return; child.kill(signal); await new Promise(resolve => child.once("exit", resolve)); processes.delete(child); }
  };
}
const browser = await chromium.launch({ executablePath: process.env.CHROME_PATH || "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", headless: true, args: ["--ignore-certificate-errors", "--allow-loopback-in-peer-connection"] });
async function page() {
  const context = await browser.newContext({ ignoreHTTPSErrors: true }); const page = await context.newPage();
  page.on("pageerror", error => failures.push(String(error))); await page.goto(origin); await page.waitForFunction(() => window.ready); return page;
}
async function boot(page, options) { await page.evaluate(options => boot(options), { identity, credential, ...options }); }
async function converge(a, b) {
  await a.waitForFunction(() => node.links.size > 0, { timeout: 25000 });
  const until = Date.now() + 25000;
  while (Date.now() < until) {
    const [x,y] = await Promise.all([a.evaluate(() => inspect()), b.evaluate(() => inspect())]);
    assert.deepEqual(x.errors, []); assert.deepEqual(y.errors, []);
    if (JSON.stringify(x.version) === JSON.stringify(y.version) && x.text === y.text) return x;
    await new Promise(resolve => setTimeout(resolve, 40));
  }
  throw new Error(`Did not converge: ${JSON.stringify(await Promise.all([a.evaluate(() => inspect()),b.evaluate(() => inspect())]))}`);
}
async function check(name, run) { await run(); reports.push(name); console.log(`PASS ${name}`); }
async function textFile(runtime, expected) {
  const until = Date.now() + 10000;
  let actual;
  while (Date.now() < until) {
    actual = await readFile(join(runtime.state, "note.not"), "utf8");
    if (actual === expected) return;
    await new Promise(resolve => setTimeout(resolve, 40));
  }
  assert.equal(actual, expected);
}
try {
  for (const transport of ["udp", "tcp", "tls"]) await check(`single Rust bin relays browser CRDT updates over TURN/${transport} with PeerNode disabled`, async () => {
    const runtime = await start(`relay-${transport}`, { tls: transport === "tls" }); const [a,b] = await Promise.all([page(),page()]);
    const signal = { url: `${runtime.ws}/signal`, token, iceTransportPolicy: "relay", turnTransport: transport };
    await boot(a, { text: "Hello 🧠", signal }); const seed = await a.evaluate(() => doc.exportSnapshot());
    await boot(b, { seed, signal });
    await Promise.all([a.evaluate(() => append(" from A")), b.evaluate(() => append(" from B"))]);
    const state = await converge(a,b); assert.ok(state.text.includes(" from A")); assert.ok(state.text.includes(" from B"));
    const candidate = await a.evaluate(async () => {
      const peer = [...signal.peers.values()].find(p => p.pc.connectionState === "connected");
      const stats = await peer.pc.getStats(); const transport = [...stats.values()].find(s => s.type === "transport" && s.selectedCandidatePairId);
      const pair = stats.get(transport.selectedCandidatePairId); return stats.get(pair.localCandidateId);
    });
    assert.equal(candidate.candidateType, "relay");
    assert.equal(candidate.relayProtocol, transport);
    assert.ok(!(await readdir(runtime.state)).includes("documents.redb"));
    await a.evaluate(() => node.close()); await b.evaluate(() => node.close()); await a.context().close(); await b.context().close(); await runtime.stop();
  });
  await check("native and browser peers exchange documents over WebRTC in the same combined-service bin", async () => {
    const runtime = await start("native-rtc", {network:true,peer:true});
    const seed = await (await fetch(`${runtime.url}/document`, {method:"POST",headers:{authorization:`Bearer ${token}`,"content-type":"application/json"},body:JSON.stringify({document:"note",credential})})).json();
    const a = await page(); await boot(a, {seed,signal:{url:`${runtime.ws}/signal`,token,iceTransportPolicy:"relay",turnTransport:"udp"}});
    await a.waitForFunction(() => node.links.size > 0, {timeout:25000});
    await a.evaluate(() => append(" via native WebRTC"));
    await a.waitForFunction(() => node.remoteDurableVersions('note').some(version => JSON.stringify(version) === JSON.stringify(doc.snapshot().version)), {timeout:15000});
    await textFile(runtime, "Hello 🧠 via native WebRTC");
    assert.deepEqual(await a.evaluate(() => errors), []);
    await a.evaluate(() => node.close()); await a.context().close(); await runtime.stop();
  });
  await check("native endpoint joins an external signaling/TURN node", async () => {
    const relay = await start("external-relay");
    const endpoint = await start("external-peer", {network:false,peer:true,signaling:`${relay.ws}/signal`});
    const seed = await (await fetch(`${endpoint.url}/document`, {method:"POST",headers:{authorization:`Bearer ${token}`,"content-type":"application/json"},body:JSON.stringify({document:"note",credential})})).json();
    const a = await page(); await boot(a, {seed,signal:{url:`${relay.ws}/signal`,token,iceTransportPolicy:"relay",turnTransport:"udp"}});
    await a.waitForFunction(() => node.links.size > 0, {timeout:25000});
    await a.evaluate(() => append(" external signaling"));
    await a.waitForFunction(() => node.remoteDurableVersions('note').some(version => JSON.stringify(version) === JSON.stringify(doc.snapshot().version)), {timeout:15000});
    assert.deepEqual(await a.evaluate(() => errors), []);
    await a.evaluate(() => node.close()); await a.context().close(); await endpoint.stop(); await relay.stop();
  });
  await check("Tiptap and CM6 share the node document across browser replicas", async () => {
    const runtime = await start("projection", {network:true,peer:true});
    const projection = await startServer(0);
    const contexts = await Promise.all([browser.newContext(),browser.newContext()]);
    try {
      const pages = await Promise.all(contexts.map(context => context.newPage()));
      for (const [index, p] of pages.entries()) {
        p.on("pageerror", error => failures.push(String(error)));
        const query = new URLSearchParams({node:runtime.url,document:"note",replica:`view-${index}`});
        const fragment = new URLSearchParams({token,credential});
        await p.goto(`http://127.0.0.1:${projection.address().port}/?${query}#${fragment}`);
        await p.waitForFunction(() => window.probe?.syncSession.inspect().joined, {timeout:25000});
      }
      const [a,b] = pages;
      await a.evaluate(() => probe.selectRich(0,5)); await a.locator("#bold").click();
      await b.waitForFunction(() => probe.inspect().source === "*Hello* 🧠" && probe.inspect().cm === probe.inspect().source);
      await b.evaluate(() => probe.revealSource(probe.inspect().source.length)); await b.keyboard.insertText(" 源码");
      await a.waitForFunction(() => probe.inspect().source.endsWith(" 源码") && probe.inspect().projection === probe.inspect().source);
      for (const p of pages) {
        assert.deepEqual(await p.evaluate(() => probe.inspect().errors), []);
        assert.deepEqual(await p.evaluate(() => probe.syncSession.inspect().errors), []);
      }
      await a.evaluate(() => probe.undo(false));
      await b.waitForFunction(() => probe.inspect().source === "Hello 🧠 源码");
      await textFile(runtime, "Hello 🧠 源码");
      for (const p of pages) await p.evaluate(() => probe.syncSession.close());
    } finally { for (const context of contexts) await context.close(); await new Promise(resolve => projection.close(resolve)); await runtime.stop(); }
  });
  await check("native peer persists browser edits and survives abrupt process termination", async () => {
    let runtime = await start("persistent", { network: false, peer: true });
    const seed = await (await fetch(`${runtime.url}/document`, { method:"POST", headers:{ authorization:`Bearer ${token}`, "content-type":"application/json" }, body:JSON.stringify({document:"note",credential}) })).json();
    const a = await page(); await boot(a, { seed, ws:{url:`${runtime.ws}/peer`,token}, profile:"persistent-browser" });
    await a.waitForFunction(() => node.links.size > 0); await a.evaluate(() => append(" durable edit"));
    await a.evaluate(() => node.flush());
    await a.waitForFunction(() => node.remoteDurableVersions('note').some(version => JSON.stringify(version) === JSON.stringify(doc.snapshot().version)), {timeout:10000});
    await runtime.stop("SIGKILL");
    await a.evaluate(() => { ws.close(); append(" offline edit"); return node.flush(); });
    const before = await a.evaluate(() => ({...inspect(),writer:doc.writerId}));
    await a.evaluate(() => node.close()); await a.reload(); await a.waitForFunction(() => window.ready);
    await boot(a, { profile:"persistent-browser" });
    assert.equal(await a.evaluate(() => doc.snapshot().text), before.text); assert.notEqual(await a.evaluate(() => doc.writerId), before.writer);
    runtime = await start("persistent", { network: false, peer: true });
    // Recovery repairs a lagging text output even if SIGKILL interrupted export.
    await textFile(runtime, "Hello 🧠 durable edit");
    const restoredSeed = await (await fetch(`${runtime.url}/document`, { method:"POST",headers:{authorization:`Bearer ${token}`,"content-type":"application/json"},body:JSON.stringify({document:"note",credential}) })).json();
    const b = await page(); await boot(b, {seed:restoredSeed,ws:{url:`${runtime.ws}/peer`,token}});
    assert.ok((await b.evaluate(() => doc.snapshot().text)).includes(" durable edit"));
    await a.evaluate(async options => { const {connectWebSocket} = await import('/node/index.mjs'); window.ws = connectWebSocket(node,options); }, {url:`${runtime.ws}/peer`,token});
    const state = await converge(a,b); assert.ok(state.text.includes(" offline edit"));
    await textFile(runtime, state.text);
    assert.equal((await fetch(`${runtime.url}/signal`)).status,404);
    await a.evaluate(() => node.close()); await b.evaluate(() => node.close()); await a.context().close(); await b.context().close(); await runtime.stop();
  });
  assert.deepEqual(failures, []);
  await mkdir(new URL("results", import.meta.url), {recursive:true}); await writeFile(new URL("results/browser.json", import.meta.url),JSON.stringify({reports,failures},null,2));
} finally {
  await browser.close(); for (const child of processes) child.kill("SIGKILL"); await new Promise(resolve=>server.close(resolve)); await rm(directory,{recursive:true,force:true});
}
