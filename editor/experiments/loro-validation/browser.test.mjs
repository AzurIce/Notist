import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile, mkdir, writeFile } from "node:fs/promises";
import { resolve, extname } from "node:path";
import { chromium } from "playwright-core";

const root = process.cwd();
const server = createServer(async (req, res) => {
  try {
    const path = resolve(root, "." + (req.url === "/" ? "/index.html" : req.url));
    if (!path.startsWith(root + "/")) { res.writeHead(403).end(); return; }
    const bytes = await readFile(path);
    res.setHeader("Content-Type", ({ ".html": "text/html", ".js": "text/javascript", ".wasm": "application/wasm" })[extname(path)] ?? "application/octet-stream");
    res.end(bytes);
  } catch { res.writeHead(404).end(); }
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const browser = await chromium.launch({
  executablePath: process.env.CHROME_BIN ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  headless: true,
});
const page = await browser.newPage({ viewport: { width: 1320, height: 1000 } });
const browserErrors = [];
page.on("pageerror", (error) => browserErrors.push(String(error)));
const results = [];
async function check(name, fn) {
  browserErrors.length = 0;
  try {
    const details = await fn();
    const pluginErrors = await page.evaluate(() => window.probe.errors);
    assert.deepEqual([...browserErrors, ...pluginErrors], []);
    results.push({ name, pass: true, details }); console.log(`PASS ${name}`);
  } catch (error) {
    const observed = await page.evaluate(() => window.probe.inspect()).catch(() => null);
    const pluginErrors = await page.evaluate(() => window.probe.errors).catch(() => []);
    results.push({ name, pass: false, error: String(error), observed, browserErrors: [...browserErrors], pluginErrors });
    console.error(`FAIL ${name}: ${error}`);
  }
}
const reset = (initial, equal = false, mergeInterval = 0) => page.evaluate(([s, e, m]) => window.probe.reset(s, e, m), [initial, equal, mergeInterval]);
const invoke = (method, ...args) => page.evaluate(([m, a]) => window.probe[m](...a), [method, args]);
function consistent(state) { for (const pane of state) assert.equal(pane.text, pane.crdt); }
try {
  await page.goto(`http://127.0.0.1:${server.address().port}`);
  await page.waitForFunction(() => window.ready);
  assert.equal(await page.evaluate(() => window.probe.version), process.env.PROBE_LATEST ? "1.16.3" : "1.16.2");
  await check("first transaction with equal initial CM6/CRDT text", async () => {
    const state = await invoke("initialTransaction");
    assert.equal(state[0].text, "abc中"); consistent(state);
  });
  await check("first keyboard input in an empty editor", async () => {
    await reset("");
    await page.locator("#a .cm-content").focus();
    await page.keyboard.type("hello");
    await page.waitForTimeout(80);
    const state = await invoke("inspect");
    assert.equal(state[0].text, "hello"); consistent(state);
  });
  await check("normal typing and two-replica sync from a preloaded document", async () => {
    await reset("abc");
    await page.locator("#a .cm-content").focus();
    await invoke("select", 0, [[3, 3]]);
    await page.keyboard.insertText("中文😀");
    const state = await invoke("synchronize"); consistent(state);
    assert.equal(state[0].text, "abc中文😀"); assert.equal(state[1].text, state[0].text);
  });
  await check("two CM6 replicas: edits, synchronization, selective undo and redo", async () => {
    await reset("abc");
    await invoke("select", 0, [[1, 1]]); await invoke("replace", 0, "中😀");
    await invoke("select", 1, [[3, 3]]); await invoke("replace", 1, "B");
    const merged = await invoke("synchronize"); consistent(merged);
    assert.equal(merged[0].text, "a中😀bcB"); assert.equal(merged[1].text, merged[0].text);
    await invoke("undo", 0);
    const undone = await invoke("synchronize"); consistent(undone);
    assert.equal(undone[0].text, "abcB"); assert.equal(undone[1].text, "abcB");
    await invoke("redo", 0);
    const redone = await invoke("synchronize"); consistent(redone);
    assert.equal(redone[0].text, merged[0].text);
  });
  await check("undo restores an emoji selection after a remote prefix insertion", async () => {
    await reset("a😀bc");
    await invoke("select", 0, [[1, 3]]); await invoke("replace", 0, "");
    await invoke("remoteInsert", 0, 0, "X");
    const state = await invoke("undo", 0); consistent(state);
    assert.equal(state[0].text, "Xa😀bc");
    assert.deepEqual(state[0].ranges, [{ anchor: 2, head: 4 }]);
  });
  await check("multi-selection replacement and undo restore every range", async () => {
    await reset("aa\nbb");
    await invoke("select", 0, [[0, 2], [3, 5]], 1);
    const edited = await invoke("replace", 0, "中"); consistent(edited);
    assert.equal(edited[0].text, "中\n中");
    const undone = await invoke("undo", 0); consistent(undone);
    assert.equal(undone[0].text, "aa\nbb");
    assert.deepEqual(undone[0].ranges, [{ anchor: 0, head: 2 }, { anchor: 3, head: 5 }]);
    assert.equal(undone[0].mainIndex, 1);
  });
  await check("control: direct UndoManager calls preserve remote edits and restore selection", async () => {
    await reset("a😀bc");
    await invoke("select", 0, [[1, 3]]); await invoke("replace", 0, "");
    await invoke("remoteInsert", 0, 0, "X");
    const undone = await invoke("directUndo", 0); consistent(undone);
    assert.equal(undone[0].text, "Xa😀bc");
    assert.deepEqual(undone[0].ranges, [{ anchor: 2, head: 4 }]);
    const redone = await invoke("directRedo", 0); consistent(redone);
    assert.equal(redone[0].text, "Xabc");
  });
  await check("control: direct UndoManager still restores every selection range", async () => {
    await reset("aa\nbb");
    await invoke("select", 0, [[0, 2], [3, 5]], 1); await invoke("replace", 0, "中");
    const undone = await invoke("directUndo", 0); consistent(undone);
    assert.equal(undone[0].text, "aa\nbb");
    assert.deepEqual(undone[0].ranges, [{ anchor: 0, head: 2 }, { anchor: 3, head: 5 }]);
    assert.equal(undone[0].mainIndex, 1);
  });
  await check("local non-CM6 transaction is visible in CM6", async () => {
    await reset("abc");
    const state = await invoke("directLocalInsert", 0, 0, "Agent: ");
    consistent(state);
  });
  for (const textFirst of [true, false]) {
    await check(`remote text + metadata transaction (textFirst=${textFirst})`, async () => {
      await reset("abc");
      const state = await invoke("mixedRemote", 0, textFirst);
      assert.equal(state[0].crdt, "Rabc"); consistent(state);
    });
  }
  await check("Chromium IME composition with a remote insertion during composition", async () => {
    await reset("AB", false, 1000);
    await page.locator("#a .cm-content").focus();
    await invoke("select", 0, [[1, 1]]);
    const cdp = await page.context().newCDPSession(page);
    await cdp.send("Input.imeSetComposition", { text: "ni", selectionStart: 2, selectionEnd: 2 });
    await page.waitForTimeout(60);
    await cdp.send("Input.imeSetComposition", { text: "你", selectionStart: 1, selectionEnd: 1 });
    await page.waitForTimeout(60);
    await invoke("remoteInsert", 0, 0, "远");
    await cdp.send("Input.insertText", { text: "你好" });
    await page.waitForTimeout(100);
    await cdp.detach();
    const state = await invoke("synchronize"); consistent(state);
    assert.equal(state[0].text, "远A你好B");
    assert.equal(state[1].text, state[0].text);
    assert.ok(state[0].composition.some((e) => e.type === "compositionstart" && e.trusted));
    // CM6 may synthesize the final compositionend after the CDP commit.
    assert.ok(state[0].composition.some((e) => e.type === "compositionend"));
    const undone = await invoke("directUndo", 0); consistent(undone);
    return {
      text: state[0].text, composition: state[0].composition,
      afterOneUndo: undone[0].text,
      knownIssue: undone[0].text !== "远AB" ? "one IME composition is split into multiple undo steps across a remote import" : undefined,
    };
  });
  await mkdir("results", { recursive: true });
  await writeFile(`results/browser${process.env.PROBE_LATEST ? "-latest" : ""}.json`, JSON.stringify({
    browser: browser.version(), binding: "loro-codemirror@0.3.3", loro: process.env.PROBE_LATEST ? "1.16.3" : "1.16.2",
    imeCoverage: "Chromium CDP composition events; not a physical macOS input-method test",
    results,
  }, null, 2));
  await reset("= Loro 验证\n\n中文与 emoji 😀\n\n双副本同步 / 撤销 / 选区");
  await invoke("select", 0, [[13, 13]]); await invoke("replace", 0, "实时编辑："); await invoke("synchronize");
  await page.waitForFunction(() => document.getElementById("state").textContent === JSON.stringify(window.probe.inspect(), null, 2));
  await page.screenshot({ path: "results/browser.png", fullPage: true });
} finally {
  await browser.close();
  await new Promise((resolve) => server.close(resolve));
}
if (results.some((r) => !r.pass)) process.exitCode = 1;
