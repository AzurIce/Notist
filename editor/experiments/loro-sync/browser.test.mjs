import { chromium } from "playwright-core";
import assert from "node:assert/strict";
import { mkdtemp, mkdir, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { startExperiment } from "./server.mjs";

const directory = await mkdtemp(join(tmpdir(), "notist-sync-browser-"));
let server = await startExperiment({ port: 0, directory, saveInterval: 150 });
const browser = await chromium.launch({ executablePath: process.env.CHROME_PATH || "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", headless: true });
const reports = [], failures = [];
const contexts = await Promise.all([browser.newContext({ viewport: { width: 1400, height: 950 } }), browser.newContext({ viewport: { width: 1400, height: 950 } })]);
const [a, b] = await Promise.all(contexts.map(context => context.newPage()));
for (const page of [a, b]) page.on("pageerror", error => failures.push(String(error)));
const inspect = page => page.evaluate(() => ({ ...window.probe.inspect(), sync: window.probe.syncSession.inspect(), version: window.probe.core.snapshot().version }));
const ready = page => page.waitForFunction(() => window.probe?.syncSession.inspect().joined && window.probe.inspect().languageReady, { timeout: 15000 });
async function converge() {
  const deadline = Date.now() + 12000;
  while (Date.now() < deadline) {
    const states = await Promise.all([inspect(a), inspect(b)]);
    if (JSON.stringify(states[0].version) === JSON.stringify(states[1].version)) {
      for (const state of states) { assert.equal(state.source, state.cm); assert.equal(state.source, state.projection); assert.deepEqual(state.errors, []); }
      assert.deepEqual(failures, []); return states;
    }
    await new Promise(resolve => setTimeout(resolve, 30));
  }
  throw new Error(`Replicas did not converge: ${JSON.stringify(await Promise.all([inspect(a), inspect(b)]))}`);
}
async function check(name, run) { await run(); reports.push(name); console.log(`PASS ${name}`); }
async function sourceAppend(page, text) {
  await page.evaluate(() => { const end = window.probe.inspect().source.length; window.probe.revealSource(end, end); });
  await page.keyboard.insertText(text);
}
try {
  await Promise.all([a.goto(`${server.url}/editor?room=browser&replica=a`), b.goto(`${server.url}/editor?room=browser&replica=b`)]);
  await Promise.all([ready(a), ready(b)]);
  await check("two independent browsers join the same history through the official WebSocket implementation", async () => {
    const states = await converge();
    assert.notEqual(states[0].sync.writer, states[1].sync.writer);
    assert.equal(states[0].sync.identity.history_id, states[1].sync.identity.history_id);
  });
  await check("rich formatting reaches both source and rich views on the other replica", async () => {
    await a.evaluate(() => { const start = window.probe.inspect().source.indexOf("world"); window.probe.selectRich(start, start + 5); });
    await a.waitForFunction(() => window.probe.rich.view.hasFocus());
    await a.locator("#bold").click();
    await b.waitForFunction(() => window.probe.inspect().source.includes("*world*"));
    await converge(); assert.equal(await b.locator(".tiptap strong").textContent(), "world");
  });
  await check("collaborative undo removes local formatting and preserves the other replica's source edit", async () => {
    await sourceAppend(b, "\n来自 B 的源码修改 🧠\n"); await converge();
    await a.locator("#undo").click();
    await b.waitForFunction(() => !window.probe.inspect().source.includes("*world*"));
    const states = await converge(); assert.ok(states[0].source.includes("来自 B 的源码修改 🧠"));
  });
  await check("offline reload preserves CRDT identity and unsent edits, then reconnect merges both directions", async () => {
    await a.locator("#network-toggle").click(); await b.locator("#network-toggle").click();
    await sourceAppend(a, "\nA 的离线记录 🐈\n"); await sourceAppend(b, "\nB 的离线记录 🌿\n");
    await a.evaluate(() => window.probe.syncSession.flush());
    const before = await inspect(a);
    await a.reload(); await a.waitForFunction(() => window.probe?.syncSession.inspect().saved);
    const after = await inspect(a);
    assert.equal(after.source, before.source);
    assert.deepEqual(after.sync.identity, before.sync.identity);
    assert.notEqual(after.sync.writer, before.sync.writer);
    assert.equal(after.sync.manualOffline, true);
    await a.locator("#network-toggle").click(); await b.locator("#network-toggle").click();
    await Promise.all([ready(a), ready(b)]);
    const states = await converge();
    for (const text of ["A 的离线记录 🐈", "B 的离线记录 🌿"]) assert.ok(states[0].source.includes(text));
  });
  await check("a saved server snapshot survives restart and hydrates a fresh browser", async () => {
    const expected = (await inspect(a)).source;
    await new Promise(resolve => setTimeout(resolve, 400));
    const port = server.http.address().port, wsPort = Number(new URL(server.wsUrl).port);
    await server.stop(); server = await startExperiment({ port, wsPort, directory, saveInterval: 150 });
    const fresh = await browser.newContext(); const page = await fresh.newPage();
    await page.goto(`${server.url}/editor?room=browser&replica=fresh`); await ready(page);
    assert.equal((await inspect(page)).source, expected);
    await fresh.close();
    await Promise.all([ready(a), ready(b)]); await converge();
  });
  await check("the two-replica experiment page loads without changing the standalone draft", async () => {
    const page = await contexts[0].newPage(); page.on("pageerror", error => failures.push(String(error)));
    await page.goto(`${server.url}/?room=dashboard`);
    await page.frameLocator("#a").locator("#network-toggle").waitFor();
    await page.frameLocator("#b").locator("#network-toggle").waitFor();
    await page.setViewportSize({ width: 1800, height: 1100 });
    await mkdir("results", { recursive: true });
    await page.screenshot({ path: "results/two-replicas.png", fullPage: true });
    assert.equal(await a.evaluate(() => localStorage.getItem("notist-source-projection-draft")), null);
    await page.close();
  });
  await writeFile("results/browser.json", JSON.stringify({ reports, failures }, null, 2));
} finally {
  await browser.close(); await server.stop(); await rm(directory, { recursive: true, force: true });
}
