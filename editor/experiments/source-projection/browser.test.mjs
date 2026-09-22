import { chromium } from "playwright-core";
import { mkdir, writeFile } from "node:fs/promises";
import assert from "node:assert/strict";
import { startServer } from "./server.mjs";

const server = await startServer(0);
const browser = await chromium.launch({ executablePath: process.env.CHROME_PATH || "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", headless: true });
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, deviceScaleFactor: 1 });
const pageErrors = [], reports = [];
page.on("pageerror", error => pageErrors.push(String(error)));
await mkdir("results", { recursive: true });
const inspect = () => page.evaluate(() => window.probe.inspect());
const reset = source => page.evaluate(source => window.probe.reset(source), source);
async function select(from, to = from) {
  await page.evaluate(([from, to]) => window.probe.selectRich(from, to), [from, to]);
  // Tiptap's focus command runs in requestAnimationFrame.
  await page.waitForFunction(() => window.probe.rich.view.hasFocus());
}
async function equal(expected) {
  const state = await inspect();
  assert.equal(state.source, expected); assert.equal(state.cm, expected); assert.equal(state.projection, expected);
  assert.deepEqual(state.errors, []); assert.deepEqual(pageErrors, []);
  return state;
}
async function check(name, run) { await run(); reports.push(name); console.log(`PASS ${name}`); }
try {
  await page.goto(`http://127.0.0.1:${server.address().port}`);
  await page.waitForFunction(() => window.probe);
  await page.waitForFunction(() => window.probe.inspect().languageReady, { timeout: 15000 });
  await check("sample uses real language core and has no diagnostics", async () => {
    const state = await inspect(); await equal(state.source); assert.deepEqual(state.diagnostics, []);
    await page.screenshot({ path: "results/split-view.png", fullPage: true });
  });
  await check("rich bold is a local source patch; comments, annotations and whitespace survive", async () => {
    const source = '// keep exactly\n#let answer = 42\n\n\nHello world.\n \n@(label: "x")\nUntouched\n';
    await reset(source); const start = source.indexOf("world"); await select(start, start + 5);
    await page.locator("#bold").click();
    const state = await equal(source.replace("world", "*world*"));
    assert.deepEqual(state.lastPatch, { from: start, to: start + 5, insert: "*world*", origin: "rich" });
    await page.evaluate(() => window.probe.undo(false)); await equal(source);
    assert.equal(await page.evaluate(() => { const { state } = window.probe.rich; return state.doc.textBetween(state.selection.from, state.selection.to); }), "world");
    await page.evaluate(() => window.probe.undo(true)); await equal(source.replace("world", "*world*"));
  });
  await check("paragraph split, continued typing, and join preserve the remaining source", async () => {
    await reset("Hello world\n \n\nTail\n"); await select(6);
    await page.keyboard.press("Enter"); await equal("Hello \n\nworld\n \n\nTail\n");
    await page.keyboard.insertText("new "); await equal("Hello \n\nnew world\n \n\nTail\n");
    await page.evaluate(() => window.probe.undo(false)); await equal("Hello \n\nworld\n \n\nTail\n");
    await select(8); await page.keyboard.press("Backspace"); await equal("Hello world\n \n\nTail\n");
  });
  await check("unfinished source remains editable and unrelated rich edits preserve it", async () => {
    await reset("A paragraph\n\n#let unfinished = (");
    await page.waitForFunction(() => window.probe.inspect().languageReady);
    assert.ok((await inspect()).diagnostics.length > 0);
    assert.equal(await page.locator(".source-island").count(), 1);
    await select(1); await page.keyboard.insertText(" real"); await equal("A real paragraph\n\n#let unfinished = (");
    await page.locator(".source-island button").click();
    await page.keyboard.insertText("#let finished = 42;"); await equal("A real paragraph\n\n#let finished = 42;");
    await page.waitForFunction(() => window.probe.inspect().languageReady);
    assert.deepEqual((await inspect()).diagnostics, []);
  });
  await check("undo order spans source and rich editors, with source-origin redo", async () => {
    await reset("Alpha beta"); await select(6, 10); await page.locator("#bold").click();
    await page.evaluate(() => window.probe.revealSource(0, 5)); await page.keyboard.insertText("Gamma"); await equal("Gamma *beta*");
    assert.equal(await page.locator(".tiptap p").textContent(), "Gamma beta");
    assert.equal(await page.locator(".tiptap strong").textContent(), "beta");
    await page.keyboard.press("Meta+z"); await equal("Alpha *beta*");
    await page.keyboard.press("Meta+z"); await equal("Alpha beta");
    await page.keyboard.press("Meta+Shift+z"); await equal("Alpha *beta*");
    await page.keyboard.press("Meta+Shift+z"); await equal("Gamma *beta*");
  });
  await check("literal markup typed in rich text becomes escaped source", async () => {
    await reset("Hello "); await select(5); await page.keyboard.insertText(" # * [ ] 🧠");
    await equal("Hello \\# \\* \\[ \\] 🧠 ");
  });
  await check("Chinese IME composition stays in place and shares one undo group", async () => {
    await reset("写在这里"); await select(2);
    const cdp = await page.context().newCDPSession(page);
    await cdp.send("Input.imeSetComposition", { text: "zhong", selectionStart: 5, selectionEnd: 5 });
    await cdp.send("Input.imeSetComposition", { text: "中文", selectionStart: 2, selectionEnd: 2 });
    await cdp.send("Input.insertText", { text: "中文" });
    await equal("写在中文这里");
    await page.evaluate(() => window.probe.undo(false)); await equal("写在这里");
    await cdp.detach();
  });
  await check("protected source blocks cannot be lost by Select All / Delete", async () => {
    await reset("Hello\n\n// keep me\n#let value = 1"); await select(0);
    await page.keyboard.press("Meta+a"); await page.keyboard.press("Backspace");
    await equal("Hello\n\n// keep me\n#let value = 1");
  });
  await check("real diagnostic byte ranges navigate correctly after CJK and emoji", async () => {
    const source = "中文 🧠\n\n#missing";
    await reset(source); await page.waitForFunction(() => window.probe.inspect().languageReady);
    const diagnostic = (await inspect()).diagnostics.find(d => d.range?.[1] > d.range?.[0]);
    assert.ok(diagnostic, "expected a ranged language diagnostic");
    await page.locator("#diagnostic-status").click();
    await page.locator("#diagnostics button").filter({ hasText: diagnostic.message }).first().click();
    const selected = await page.evaluate(() => { const { state } = window.probe.cm; return state.sliceDoc(state.selection.main.from, state.selection.main.to); });
    assert.equal(selected, Buffer.from(source).subarray(...diagnostic.range).toString());
    assert.ok(selected.includes("missing"));
  });
  await check("document-only mode and local draft persistence", async () => {
    await reset(); await page.locator("#document-only").click();
    assert.equal(await page.locator(".source-pane").isVisible(), false);
    await page.locator("#toast").waitFor({ state: "hidden" });
    await page.screenshot({ path: "results/document-view.png", fullPage: true });
    const source = (await inspect()).source;
    await page.reload(); await page.waitForFunction(() => window.probe);
    await equal(source);
  });
  await check("external kernel transactions and remote imports update both views and share undo", async () => {
    await reset("Alpha beta");
    await page.evaluate(() => {
      const core = window.probe.core;
      core.transact({ expectedVersion: core.snapshot().version, origin: "agent", edits: [{ from: 0, to: 5, insert: "Gamma" }] });
    });
    await equal("Gamma beta");
    assert.equal(await page.locator(".tiptap p").textContent(), "Gamma beta");
    await page.evaluate(() => {
      const core = window.probe.core, replica = window.probe.makeReplica();
      replica.transact({ expectedVersion: replica.snapshot().version, origin: "remote", edits: [{ from: 10, to: 10, insert: " 远端🧠" }] });
      core.import(replica.exportUpdatesSince(core.snapshot().version), "other-instance");
      replica.dispose();
    });
    await equal("Gamma beta 远端🧠");
    assert.equal(await page.locator(".tiptap p").textContent(), "Gamma beta 远端🧠");
    await page.evaluate(() => window.probe.undo(false));
    await equal("Alpha beta 远端🧠");
    await page.evaluate(() => window.probe.undo(true));
    await equal("Gamma beta 远端🧠");
  });
  await check("undo restores the rich selection after a remote prefix insertion", async () => {
    await reset("Hello world."); await select(6, 11); await page.locator("#bold").click();
    await page.evaluate(() => {
      const core = window.probe.core, replica = window.probe.makeReplica();
      replica.transact({ expectedVersion: replica.snapshot().version, origin: "remote", edits: [{ from: 0, to: 0, insert: "远🧠" }] });
      core.import(replica.exportUpdatesSince(core.snapshot().version)); replica.dispose();
    });
    await equal("远🧠Hello *world*.");
    await page.evaluate(() => window.probe.undo(false));
    await equal("远🧠Hello world.");
    assert.equal(await page.evaluate(() => { const { state } = window.probe.rich; return state.doc.textBetween(state.selection.from, state.selection.to); }), "world");
  });
  await writeFile("results/browser.json", JSON.stringify({ passed: reports, pageErrors }, null, 2));
} catch (error) {
  await page.screenshot({ path: "results/failure.png", fullPage: true });
  console.error(JSON.stringify({ state: await inspect().catch(() => null), pageErrors }, null, 2));
  throw error;
} finally { await browser.close(); server.close(); }
